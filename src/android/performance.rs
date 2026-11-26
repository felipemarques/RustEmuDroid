//! Per-profile performance engine: a staged Windows engine described by a
//! manifest written by this project, plus a preset of individually switchable
//! host and AVD settings. Every gate in the patched engine defaults to off, so
//! the client only sets what the profile asked for and the manifest supports.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    Gpu, Locale, Result, RuntimeKind, Sdk,
    engine_folder::{StageError, find_stage, probe, read_error},
};

/// Written by us next to the engine's `sdk` folder.
pub(crate) const MANIFEST: &str = "rustemudroid-engine.json";
const MANIFEST_KIND: &str = "rustemudroid-performance-engine";
/// Original `config.ini` lines replaced by the preset, kept in the AVD folder.
const BACKUP: &str = "rustemudroid-performance.json";

const EMULATOR: &str = "emulator/emulator.exe";
const QEMU: &str = "emulator/qemu/windows-x86_64/qemu-system-x86_64.exe";
const GFXSTREAM: &str = "emulator/lib64/libgfxstream_backend.dll";

pub(crate) const KERNEL_PARAMETERS: &str =
    "mitigations=off init_on_alloc=0 transparent_hugepage=always";
const ASG_SPIN_US: &str = "3000";
/// vCPUs on the last physical cores, never on core 0 (where Windows
/// concentrates interrupts); the remaining cores run the engine's other threads.
const VCPU_PLACEMENT: &str = "sets-high";
const GLTRANSPORT_KEY: &str = "hw.gltransport";
const KERNEL_KEY: &str = "kernel.parameters";

/// Every variable with this prefix is removed from the inherited environment
/// before a performance-engine launch, so only the profile's selection reaches
/// the engine, including gates added by later engine builds.
const QA_PREFIX: &str = "RUSTEMUDROID_QA_";

/// The gates known in the patched 37.3.10 tree, also cleared explicitly (the
/// launch clears every inherited `RUSTEMUDROID_QA_*` variable, see [`QA_PREFIX`]).
const QA_GATES: [&str; 17] = [
    "RUSTEMUDROID_QA_WHPX_SYNC_TSC",
    "RUSTEMUDROID_QA_LAPIC_FASTPATH",
    "RUSTEMUDROID_QA_KVM_PV_EOI",
    "RUSTEMUDROID_QA_VULKAN_CACHED_HOST",
    "RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL",
    "RUSTEMUDROID_QA_ASG_SPIN_US",
    "RUSTEMUDROID_QA_VCPU_PLACEMENT",
    "RUSTEMUDROID_QA_VCPU_CORES",
    "RUSTEMUDROID_QA_VCPU_PRIORITY",
    "RUSTEMUDROID_QA_WHPX_EXIT_STATS",
    "RUSTEMUDROID_QA_RENDER_SPIN_US",
    "RUSTEMUDROID_QA_PIPE_RECV_WAIT_US",
    "RUSTEMUDROID_QA_PIPE_LOCKLESS",
    "RUSTEMUDROID_QA_PRERUN_LOCKLESS",
    "RUSTEMUDROID_QA_HALT_POLL_US",
    "RUSTEMUDROID_QA_READBACK_UNLOCK",
    "RUSTEMUDROID_QA_RAM_LARGE_PAGES",
];

/// Followed by ` init` or ` reset`; see [`clock_confirmed`].
const PROOF_TSC: &str = "WHPX TSC QA: synchronized";
const PROOF_LAPIC: &str = "RustEmuDroid QA: LAPIC MMIO fast path active (32-bit MOV)";
const PROOF_PV_EOI: &str = "RustEmuDroid QA: KVM PV EOI active";
pub(super) const PROOF_CACHED_HOST: &str =
    "RustEmuDroid QA: requested cached Vulkan host allocations";
const PROOF_DEVICE_LOCAL: &str =
    "RustEmuDroid QA: cached mappable device-local Vulkan allocations active (v2)";
const PROOF_ASG_TRANSPORT: &str = "androidboot.hardware.gltransport=asg";
const PROOF_ASG_SPIN: &str = "RustEmuDroid QA: ASG consumer spin active";
const PROOF_NATIVE_SWAPCHAIN: &str = "useVulkanNativeSwapchain: true";
const PROOF_VCPU: &str = "RustEmuDroid QA: vCPU placement mode=";

/// Names an engine manifest can list. Unknown names (including the older
/// `vcpu_placement`, which meant the low-core `sets` mode) are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Capability {
    TscSync,
    LapicFastpath,
    KvmPvEoi,
    VulkanCachedHost,
    VulkanCachedDeviceLocalV2,
    AsgSpin,
    VulkanNativeSwapchain,
    VcpuPlacementHigh,
    GuestKernelParameters,
}

impl Capability {
    pub(crate) const ALL: [Self; 9] = [
        Self::TscSync,
        Self::LapicFastpath,
        Self::KvmPvEoi,
        Self::VulkanCachedHost,
        Self::VulkanCachedDeviceLocalV2,
        Self::AsgSpin,
        Self::VulkanNativeSwapchain,
        Self::VcpuPlacementHigh,
        Self::GuestKernelParameters,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::TscSync => "tsc_sync",
            Self::LapicFastpath => "lapic_fastpath",
            Self::KvmPvEoi => "kvm_pv_eoi",
            Self::VulkanCachedHost => "vulkan_cached_host",
            Self::VulkanCachedDeviceLocalV2 => "vulkan_cached_device_local_v2",
            Self::AsgSpin => "asg_spin",
            Self::VulkanNativeSwapchain => "vulkan_native_swapchain",
            Self::VcpuPlacementHigh => "vcpu_placement_high",
            Self::GuestKernelParameters => "guest_kernel_parameters",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.name() == name)
    }
}

/// One engine gate of the preset. The settings dialog groups the three CPU
/// virtualization gates under one switch; the preferences file keeps them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tuning {
    ClockSync,
    LapicFastpath,
    KvmPvEoi,
    VulkanMemoryCache,
    AsgTransport,
    VulkanComposition,
    DedicatedVcpuCores,
    GuestKernelParameters,
}

impl Tuning {
    pub(crate) const ALL: [Self; 8] = [
        Self::ClockSync,
        Self::LapicFastpath,
        Self::KvmPvEoi,
        Self::VulkanMemoryCache,
        Self::AsgTransport,
        Self::VulkanComposition,
        Self::DedicatedVcpuCores,
        Self::GuestKernelParameters,
    ];

    /// Shown as one "CPU virtualization" switch.
    #[cfg_attr(not(all(windows, feature = "ui")), allow(dead_code))]
    pub(crate) const CPU_VIRTUALIZATION: [Self; 3] =
        [Self::ClockSync, Self::LapicFastpath, Self::KvmPvEoi];

    pub(crate) const fn capability(self) -> Capability {
        match self {
            Self::ClockSync => Capability::TscSync,
            Self::LapicFastpath => Capability::LapicFastpath,
            Self::KvmPvEoi => Capability::KvmPvEoi,
            Self::VulkanMemoryCache => Capability::VulkanCachedHost,
            Self::AsgTransport => Capability::AsgSpin,
            Self::VulkanComposition => Capability::VulkanNativeSwapchain,
            Self::DedicatedVcpuCores => Capability::VcpuPlacementHigh,
            Self::GuestKernelParameters => Capability::GuestKernelParameters,
        }
    }

    pub(crate) fn enabled(self, options: &PerformanceOptions) -> bool {
        match self {
            Self::ClockSync => options.clock_sync,
            Self::LapicFastpath => options.lapic_fastpath,
            Self::KvmPvEoi => options.kvm_pv_eoi,
            Self::VulkanMemoryCache => options.vulkan_memory_cache,
            Self::AsgTransport => options.asg_transport,
            Self::VulkanComposition => options.vulkan_composition,
            Self::DedicatedVcpuCores => options.dedicated_vcpu_cores,
            Self::GuestKernelParameters => options.guest_kernel_parameters,
        }
    }

    #[cfg_attr(not(all(windows, feature = "ui")), allow(dead_code))]
    pub(crate) fn enabled_mut(self, options: &mut PerformanceOptions) -> &mut bool {
        match self {
            Self::ClockSync => &mut options.clock_sync,
            Self::LapicFastpath => &mut options.lapic_fastpath,
            Self::KvmPvEoi => &mut options.kvm_pv_eoi,
            Self::VulkanMemoryCache => &mut options.vulkan_memory_cache,
            Self::AsgTransport => &mut options.asg_transport,
            Self::VulkanComposition => &mut options.vulkan_composition,
            Self::DedicatedVcpuCores => &mut options.dedicated_vcpu_cores,
            Self::GuestKernelParameters => &mut options.guest_kernel_parameters,
        }
    }

    /// Short name used in the startup status.
    pub(crate) const fn short_name(self, locale: Locale) -> &'static str {
        match self {
            Self::ClockSync => locale.pick(
                "relógio sincronizado",
                "synchronized clock",
                "reloj sincronizado",
            ),
            Self::LapicFastpath => locale.pick(
                "acesso rápido ao APIC",
                "fast APIC access",
                "acceso rápido al APIC",
            ),
            Self::KvmPvEoi => locale.pick(
                "fim de interrupção paravirtualizado",
                "paravirtualized end of interrupt",
                "fin de interrupción paravirtualizado",
            ),
            Self::VulkanMemoryCache => locale.pick(
                "cache de memória Vulkan",
                "Vulkan memory cache",
                "caché de memoria Vulkan",
            ),
            Self::AsgTransport => locale.pick(
                "transporte gráfico compartilhado",
                "shared graphics transport",
                "transporte gráfico compartido",
            ),
            Self::VulkanComposition => locale.pick(
                "composição Vulkan",
                "Vulkan composition",
                "composición Vulkan",
            ),
            Self::DedicatedVcpuCores => {
                locale.pick("núcleos dedicados", "dedicated cores", "núcleos dedicados")
            }
            Self::GuestKernelParameters => locale.pick(
                "parâmetros do kernel",
                "kernel parameters",
                "parámetros del kernel",
            ),
        }
    }
}

/// Saved per profile. Missing fields take the conservative defaults, so older
/// preference files and future fields both load.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "ui", serde(default))]
pub struct PerformanceOptions {
    pub clock_sync: bool,
    pub lapic_fastpath: bool,
    pub kvm_pv_eoi: bool,
    pub vulkan_memory_cache: bool,
    pub asg_transport: bool,
    pub vulkan_composition: bool,
    pub dedicated_vcpu_cores: bool,
    /// Weakens guest security. Never on unless the user opts in.
    pub guest_kernel_parameters: bool,
}

impl Default for PerformanceOptions {
    fn default() -> Self {
        Self {
            clock_sync: true,
            lapic_fastpath: true,
            kvm_pv_eoi: true,
            vulkan_memory_cache: true,
            asg_transport: true,
            vulkan_composition: true,
            dedicated_vcpu_cores: true,
            guest_kernel_parameters: false,
        }
    }
}

/// `sets-high` needs more physical cores than the profile's vCPUs plus two
/// (the engine itself falls back below vCPUs + 2).
pub(crate) fn placement_fits(physical_cores: Option<usize>, vcpus: usize) -> bool {
    physical_cores.is_some_and(|cores| cores > vcpus + 2)
}

/// vCPU count of an AVD (`hw.cpu.ncore`), 4 when absent as in profiles created here.
pub(crate) fn avd_vcpus(avd: &Path) -> usize {
    fs::read_to_string(avd.join("config.ini"))
        .ok()
        .and_then(|config| {
            super::property(&config, "hw.cpu.ncore").and_then(|value| value.parse().ok())
        })
        .unwrap_or(4)
}

/// Physical cores of the host (all processor groups), read through
/// GetLogicalProcessorInformationEx (resolved at run time, no extra crate feature).
pub(crate) fn physical_cores() -> Option<usize> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        type Query = unsafe extern "system" fn(i32, *mut u8, *mut u32) -> i32;
        let kernel32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        // SAFETY: kernel32 is always loaded; the signature matches the documented export.
        let query: Query = unsafe {
            let module = GetModuleHandleW(kernel32.as_ptr());
            if module.is_null() {
                return None;
            }
            std::mem::transmute(GetProcAddress(
                module,
                c"GetLogicalProcessorInformationEx".as_ptr().cast(),
            )?)
        };
        let mut size = 0u32;
        // SAFETY: a null buffer only queries the required size.
        unsafe { query(0, std::ptr::null_mut(), &mut size) };
        let mut buffer = vec![0u8; size as usize];
        // SAFETY: the buffer has the size the system asked for. 0 = RelationProcessorCore.
        if size == 0 || unsafe { query(0, buffer.as_mut_ptr(), &mut size) } == 0 {
            return None;
        }
        // Each record starts with Relationship (u32) and Size (u32).
        let (mut offset, mut cores) = (0usize, 0usize);
        while offset + 8 <= size as usize {
            let field = |at: usize| u32::from_le_bytes(buffer[at..at + 4].try_into().unwrap());
            let length = field(offset + 4) as usize;
            if length == 0 {
                break;
            }
            cores += usize::from(field(offset) == 0);
            offset += length;
        }
        (cores > 0).then_some(cores)
    }
    #[cfg(not(windows))]
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct PerformanceEngine {
    pub path: PathBuf,
    #[cfg_attr(feature = "ui", serde(default))]
    pub options: PerformanceOptions,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub(crate) engine_version: String,
    pub(crate) build_id: String,
    /// Paths relative to the engine's `sdk` folder and their SHA-256.
    files: Vec<(String, String)>,
    pub(crate) capabilities: Vec<Capability>,
}

fn sha256_field(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

fn safe_relative(path: &str) -> bool {
    path.starts_with("emulator/")
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

pub(crate) fn parse_manifest(text: &str, locale: Locale) -> Result<Manifest> {
    let invalid = locale.pick(
        "O manifesto do motor de desempenho está incompleto ou é de outra versão.",
        "The performance engine manifest is incomplete or uses another format.",
        "El manifiesto del motor de rendimiento está incompleto o usa otro formato.",
    );
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("{invalid} ({error})"))?;
    if value["schema"].as_u64() != Some(1) || value["kind"].as_str() != Some(MANIFEST_KIND) {
        return Err(invalid.into());
    }
    let text_field = |name: &str| {
        value[name]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .ok_or(invalid)
    };
    let engine_version = text_field("engine_version")?;
    let build_id = text_field("build_id")?;
    let mut files = Vec::new();
    for (field, path) in [
        ("emulator_sha256", EMULATOR),
        ("qemu_sha256", QEMU),
        ("gfxstream_sha256", GFXSTREAM),
    ] {
        files.push((path.to_owned(), sha256_field(&value[field]).ok_or(invalid)?));
    }
    // Rebuilt helper libraries that ship with the patched modules.
    if let Some(extra) = value.get("extra_sha256") {
        for (path, hash) in extra.as_object().ok_or(invalid)? {
            if !safe_relative(path)
                || files
                    .iter()
                    .any(|(known, _)| known.eq_ignore_ascii_case(path))
            {
                return Err(invalid.into());
            }
            files.push((path.clone(), sha256_field(hash).ok_or(invalid)?));
        }
    }
    let mut capabilities = Vec::new();
    for name in value["capabilities"].as_array().ok_or(invalid)? {
        capabilities.extend(Capability::parse(name.as_str().ok_or(invalid)?));
    }
    capabilities.sort();
    capabilities.dedup();
    Ok(Manifest {
        engine_version,
        build_id,
        files,
        capabilities,
    })
}

fn stage_directory(path: &Path) -> std::result::Result<&Path, StageError<'_>> {
    find_stage(path, MANIFEST, probe)
}

pub(crate) fn read_manifest(path: &Path, locale: Locale) -> Result<(PathBuf, Manifest)> {
    let stage = stage_directory(path).map_err(|error| match error {
        StageError::Missing => format!(
            "{} {MANIFEST}",
            locale.pick(
                "Escolha a pasta do motor que contém",
                "Choose the engine folder containing",
                "Elige la carpeta del motor que contiene",
            )
        ),
        StageError::Unreadable(folder, error) => read_error(folder, &error, locale),
    })?;
    let manifest = stage.join(MANIFEST);
    let text =
        fs::read_to_string(&manifest).map_err(|error| read_error(&manifest, &error, locale))?;
    Ok((stage.to_owned(), parse_manifest(&text, locale)?))
}

/// Read without hashing, for the settings dialog.
#[cfg_attr(not(all(windows, feature = "ui")), allow(dead_code))]
pub(crate) fn manifest_capabilities(path: &Path) -> Option<Vec<Capability>> {
    read_manifest(path, Locale::default())
        .ok()
        .map(|(_, manifest)| manifest.capabilities)
}

/// Lets the settings dialog notice an edited manifest.
#[cfg_attr(not(all(windows, feature = "ui")), allow(dead_code))]
pub(crate) fn manifest_modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(stage_directory(path).ok()?.join(MANIFEST))
        .and_then(|meta| meta.modified())
        .ok()
}

pub(crate) struct Selected {
    pub(crate) sdk: Sdk,
    pub(crate) manifest: Manifest,
}

pub(crate) fn select(path: &Path, sdk_image: bool, gpu: Gpu, locale: Locale) -> Result<Selected> {
    if !cfg!(all(windows, target_arch = "x86_64"))
        || !sdk_image
        || !matches!(gpu, Gpu::Host | Gpu::Auto)
    {
        return Err(locale.pick(
            "O motor de desempenho exige Windows x64, uma imagem do Android SDK e GPU do computador ou automática.",
            "The performance engine requires Windows x64, an Android SDK image and host or automatic GPU mode.",
            "El motor de rendimiento requiere Windows x64, una imagen del SDK de Android y GPU del equipo o automática.",
        ).into());
    }
    let (stage, manifest) = read_manifest(path, locale)?;
    let root = stage.join("sdk");
    verify(&root, &manifest, locale)?;
    Ok(Selected {
        sdk: Sdk::from_root(root, RuntimeKind::Sdk, locale)?,
        manifest,
    })
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut hash = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn verify(root: &Path, manifest: &Manifest, locale: Locale) -> Result<()> {
    for (relative, expected) in &manifest.files {
        let path = root.join(relative);
        let actual = sha256_file(&path).map_err(|error| read_error(&path, &error, locale))?;
        if actual != *expected {
            return Err(format!(
                "{} {}",
                locale.pick(
                    "O SHA-256 não confere com o manifesto:",
                    "SHA-256 does not match the manifest:",
                    "El SHA-256 no coincide con el manifiesto:",
                ),
                path.display()
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn verified_message(manifest: &Manifest, locale: Locale) -> String {
    format!(
        "{} {} ({}).",
        locale.pick(
            "Motor de desempenho verificado:",
            "Performance engine verified:",
            "Motor de rendimiento verificado:",
        ),
        manifest.engine_version,
        manifest.build_id
    )
}

pub(crate) fn fallback_message(locale: Locale, error: &str) -> String {
    format!(
        "{} {error}",
        locale.pick(
            "O motor de desempenho não passou na verificação. O Android vai iniciar com o motor padrão.",
            "The performance engine failed verification. Android will start with the default engine.",
            "El motor de rendimiento no ha superado la verificación. Android se iniciará con el motor predeterminado.",
        )
    )
}

/// What this boot actually requests: enabled in the profile and listed by the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) tunings: Vec<Tuning>,
    /// Enabled and listed by the engine, but not requested on this computer.
    pub(crate) held_back: Vec<Tuning>,
    device_local: bool,
}

impl Plan {
    /// `placement_fits` comes from [`placement_fits`] for this host and profile.
    pub(crate) fn new(
        options: &PerformanceOptions,
        capabilities: &[Capability],
        placement_fits: bool,
    ) -> Self {
        let (tunings, held_back): (Vec<_>, Vec<_>) = Tuning::ALL
            .into_iter()
            .filter(|tuning| tuning.enabled(options) && capabilities.contains(&tuning.capability()))
            .partition(|tuning| *tuning != Tuning::DedicatedVcpuCores || placement_fits);
        let device_local = tunings.contains(&Tuning::VulkanMemoryCache)
            && capabilities.contains(&Capability::VulkanCachedDeviceLocalV2);
        Self {
            tunings,
            held_back,
            device_local,
        }
    }

    fn has(&self, tuning: Tuning) -> bool {
        self.tunings.contains(&tuning)
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        // Known gates too, in case something set them on the command itself.
        for gate in inherited_qa_gates(env::vars_os().map(|(name, _)| name))
            .into_iter()
            .chain(QA_GATES.map(OsString::from))
        {
            command.env_remove(gate);
        }
        if self.has(Tuning::ClockSync) {
            command.env("RUSTEMUDROID_QA_WHPX_SYNC_TSC", "1");
        }
        if self.has(Tuning::LapicFastpath) {
            command.env("RUSTEMUDROID_QA_LAPIC_FASTPATH", "1");
        }
        if self.has(Tuning::KvmPvEoi) {
            command.env("RUSTEMUDROID_QA_KVM_PV_EOI", "1");
        }
        if self.has(Tuning::VulkanMemoryCache) {
            command.env("RUSTEMUDROID_QA_VULKAN_CACHED_HOST", "1");
            if self.device_local {
                command.env("RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL", "2");
            }
        }
        if self.has(Tuning::AsgTransport) {
            command.env("RUSTEMUDROID_QA_ASG_SPIN_US", ASG_SPIN_US);
        }
        if self.has(Tuning::DedicatedVcpuCores) {
            command.env("RUSTEMUDROID_QA_VCPU_PLACEMENT", VCPU_PLACEMENT);
        }
    }

    /// Emulator features this plan turns on, for the launch's single
    /// `-feature` argument.
    pub(crate) fn features(&self) -> Vec<&'static str> {
        if self.has(Tuning::VulkanComposition) {
            vec!["VulkanNativeSwapchain"]
        } else {
            Vec::new()
        }
    }

    pub(crate) fn needs_guest_cmdline(&self) -> bool {
        self.has(Tuning::GuestKernelParameters)
    }

    /// Requested settings whose proof is missing from the engine log or, for
    /// the kernel parameters, from the guest's /proc/cmdline.
    pub(crate) fn inactive(&self, log: &str, guest_cmdline: Option<&str>) -> Vec<Tuning> {
        self.tunings
            .iter()
            .copied()
            .filter(|tuning| match tuning {
                Tuning::ClockSync => !clock_confirmed(log),
                Tuning::LapicFastpath => !log.contains(PROOF_LAPIC),
                Tuning::KvmPvEoi => !log.contains(PROOF_PV_EOI),
                Tuning::VulkanMemoryCache => {
                    !log.contains(PROOF_CACHED_HOST)
                        || (self.device_local && !log.contains(PROOF_DEVICE_LOCAL))
                }
                Tuning::AsgTransport => {
                    !log.contains(PROOF_ASG_TRANSPORT) || !log.contains(PROOF_ASG_SPIN)
                }
                Tuning::VulkanComposition => !log.contains(PROOF_NATIVE_SWAPCHAIN),
                Tuning::DedicatedVcpuCores => !placement_confirmed(log),
                Tuning::GuestKernelParameters => !guest_cmdline.is_some_and(|cmdline| {
                    KERNEL_PARAMETERS
                        .split(' ')
                        .all(|parameter| cmdline.split_whitespace().any(|word| word == parameter))
                }),
            })
            .collect()
    }

    /// The startup status: what is active, what the engine did not confirm
    /// (with its own warning lines) and what this computer can not use.
    pub(crate) fn status(
        &self,
        inactive: &[Tuning],
        engine_log: &str,
        locale: Locale,
        log: &Path,
    ) -> String {
        let mut text = self.summary(inactive, locale, log);
        if !self.held_back.is_empty() {
            text.push_str(&format!(
                " {} {}.",
                locale.pick(
                    "Não usado neste computador (núcleos físicos insuficientes):",
                    "Not used on this computer (not enough physical cores):",
                    "No se usa en este equipo (núcleos físicos insuficientes):",
                ),
                self.held_back
                    .iter()
                    .map(|tuning| tuning.short_name(locale))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.has(Tuning::DedicatedVcpuCores) {
            for note in placement_warnings(engine_log) {
                text.push_str(&format!(
                    " {} {note}",
                    locale.pick("Motor:", "Engine:", "Motor:")
                ));
            }
        }
        text
    }

    fn summary(&self, inactive: &[Tuning], locale: Locale, log: &Path) -> String {
        let names = |list: &mut dyn Iterator<Item = Tuning>| {
            list.map(|tuning| tuning.short_name(locale))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if self.tunings.is_empty() {
            return locale
                .pick(
                    "Motor de desempenho em uso sem ajustes selecionados.",
                    "Performance engine in use with no settings selected.",
                    "Motor de rendimiento en uso sin ajustes seleccionados.",
                )
                .into();
        }
        let active = names(
            &mut self
                .tunings
                .iter()
                .copied()
                .filter(|t| !inactive.contains(t)),
        );
        if inactive.is_empty() {
            return format!(
                "{} {active}.",
                locale.pick(
                    "Motor de desempenho ativo:",
                    "Performance engine active:",
                    "Motor de rendimiento activo:",
                )
            );
        }
        format!(
            "{} {}. {} {}. {} {}",
            locale.pick(
                "Não confirmado no log do motor (isso não significa que esteja desligado; motores mais antigos só gravam algumas linhas no fim):",
                "Not confirmed in the engine log (this does not mean it is off; older engines write some lines only at exit):",
                "No confirmado en el registro del motor (no significa que esté desactivado; los motores anteriores escriben algunas líneas solo al cerrarse):",
            ),
            names(&mut inactive.iter().copied()),
            locale.pick("Confirmados:", "Confirmed:", "Confirmados:"),
            if active.is_empty() {
                "-"
            } else {
                active.as_str()
            },
            locale.pick("Consulte", "See", "Consulta"),
            log.display()
        )
    }
}

/// Names to clear from the inherited environment: every `RUSTEMUDROID_QA_*`
/// (case-insensitive, as Windows treats variable names).
fn inherited_qa_gates(names: impl Iterator<Item = OsString>) -> Vec<OsString> {
    names
        .filter(|name| {
            name.to_str().is_some_and(|name| {
                name.len() >= QA_PREFIX.len()
                    && name.as_bytes()[..QA_PREFIX.len()].eq_ignore_ascii_case(QA_PREFIX.as_bytes())
            })
        })
        .collect()
}

/// Only `WHPX TSC QA: synchronized init` / `synchronized reset` count.
fn clock_confirmed(log: &str) -> bool {
    log.lines().any(|line| {
        line.split_once(PROOF_TSC).is_some_and(|(_, rest)| {
            rest.strip_prefix(' ')
                .and_then(|rest| rest.split_whitespace().next())
                .is_some_and(|word| word == "init" || word == "reset")
        })
    })
}

/// How long proof lines are awaited after Android reports boot completion.
const CONFIRM_WINDOW: Duration = Duration::from_secs(10);
/// Reads of the guest's /proc/cmdline within that window.
const CMDLINE_TRIES: u8 = 4;

/// Post-boot confirmation that never holds back the boot: checked once when
/// Android reports boot completion and then from the session loop, until
/// every option is confirmed, the window closes or the VM stops. The caller
/// posts one status message from the first `Some`.
pub(crate) struct Confirmation {
    deadline: Instant,
    cmdline: Option<String>,
    cmdline_tries: u8,
}

impl Confirmation {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            deadline: now + CONFIRM_WINDOW,
            cmdline: None,
            cmdline_tries: 0,
        }
    }

    /// `last`: the VM stopped or the session is ending, so this is the final
    /// look (the guest is not asked again). Returns the options still without
    /// proof once the result is final.
    pub(crate) fn check(
        &mut self,
        plan: &Plan,
        now: Instant,
        log: &str,
        read_cmdline: &mut dyn FnMut() -> Option<String>,
        last: bool,
    ) -> Option<Vec<Tuning>> {
        if plan.needs_guest_cmdline()
            && self.cmdline.is_none()
            && self.cmdline_tries < CMDLINE_TRIES
            && !last
        {
            self.cmdline_tries += 1;
            self.cmdline = read_cmdline();
        }
        let inactive = plan.inactive(log, self.cmdline.as_deref());
        (inactive.is_empty() || last || now >= self.deadline).then_some(inactive)
    }
}

/// The placement line must name `sets-high` and print a vCPU mapping. On
/// fallback the engine prints another mode (`ideal`, or the automatic order).
fn placement_confirmed(log: &str) -> bool {
    log.lines().any(|line| {
        line.split_once(PROOF_VCPU).is_some_and(|(_, rest)| {
            rest.split_once(' ').is_some_and(|(mode, mapping)| {
                mode == VCPU_PLACEMENT
                    && mapping
                        .strip_prefix("vcpu->cpus: 0->{")
                        .is_some_and(|cores| cores.contains('}'))
            })
        })
    })
}

/// The engine's own explanation when it can not place the vCPUs as asked.
fn placement_warnings(log: &str) -> Vec<&str> {
    const MARKERS: [&str; 7] = [
        "needs more cores than vCPUs",
        "needs CPU sets",
        "vCPU placement off",
        "unknown RUSTEMUDROID_QA_VCPU_PLACEMENT",
        "RUSTEMUDROID_QA_VCPU_CORES=",
        "SetProcessDefaultCpuSets failed",
        "SetThreadSelectedCpuSets vCPU",
    ];
    let mut notes: Vec<&str> = Vec::new();
    for line in log.lines() {
        if let Some((_, note)) = line.split_once("RustEmuDroid QA: ")
            && MARKERS.iter().any(|marker| note.contains(marker))
            && !notes.contains(&note.trim())
            && notes.len() < 3
        {
            notes.push(note.trim());
        }
    }
    notes
}

/// The two AVD keys owned by the preset. `None` restores the profile's own value.
pub(crate) fn avd_settings(plan: Option<&Plan>) -> [(&'static str, Option<&'static str>); 2] {
    let has = |tuning| plan.is_some_and(|plan| plan.has(tuning));
    [
        (GLTRANSPORT_KEY, has(Tuning::AsgTransport).then_some("asg")),
        (
            KERNEL_KEY,
            has(Tuning::GuestKernelParameters).then_some(KERNEL_PARAMETERS),
        ),
    ]
}

type Backup = BTreeMap<String, Option<String>>;

struct Line {
    text: String,
    end: String,
}

fn split_lines(contents: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut rest = contents;
    while !rest.is_empty() {
        let (line, tail) = rest.split_at(rest.find('\n').map_or(rest.len(), |at| at + 1));
        let (text, end) = if let Some(text) = line.strip_suffix("\r\n") {
            (text, "\r\n")
        } else if let Some(text) = line.strip_suffix('\n') {
            (text, "\n")
        } else {
            (line, "")
        };
        lines.push(Line {
            text: text.into(),
            end: end.into(),
        });
        rest = tail;
    }
    lines
}

fn find_key(lines: &[Line], key: &str) -> Option<usize> {
    lines.iter().position(|line| {
        line.text
            .split_once('=')
            .is_some_and(|(name, _)| name.trim() == key)
    })
}

fn insert_line(lines: &mut Vec<Line>, text: String) {
    let end = lines
        .iter()
        .find(|line| !line.end.is_empty())
        .map_or("\n".to_owned(), |line| line.end.clone());
    match lines.last_mut() {
        // Keep a file without a final line break in that form.
        Some(last) if last.end.is_empty() => {
            last.end = end;
            lines.push(Line {
                text,
                end: String::new(),
            });
        }
        _ => lines.push(Line { text, end }),
    }
}

fn remove_key(lines: &mut Vec<Line>, key: &str) {
    if let Some(index) = find_key(lines, key) {
        let removed = lines.remove(index);
        if removed.end.is_empty()
            && let Some(last) = lines.last_mut()
        {
            last.end.clear();
        }
    }
}

fn replace_key(lines: &mut Vec<Line>, key: &str, text: String) {
    match find_key(lines, key) {
        Some(index) => lines[index].text = text,
        None => insert_line(lines, text),
    }
}

fn parse_backup(bytes: &[u8]) -> std::result::Result<Backup, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if value["schema"].as_u64() != Some(1) {
        return Err(format!("schema {}", value["schema"]));
    }
    serde_json::from_value(value["original_lines"].clone()).map_err(|error| error.to_string())
}

/// The record of original lines can not be used; nothing was changed.
fn backup_error(path: &Path, detail: &str, locale: Locale) -> String {
    format!(
        "{} {} ({detail}). {}",
        locale.pick(
            "O registro das linhas originais do config.ini está ilegível ou danificado:",
            "The record of the original config.ini lines is unreadable or damaged:",
            "El registro de las líneas originales de config.ini está ilegible o dañado:",
        ),
        path.display(),
        locale.pick(
            "Nada foi alterado. Apagar rustemudroid-performance.json descarta as linhas originais salvas (o config.ini fica como está agora).",
            "Nothing was changed. Deleting rustemudroid-performance.json discards the saved original lines (config.ini stays as it is now).",
            "No se ha cambiado nada. Borrar rustemudroid-performance.json descarta las líneas originales guardadas (config.ini se queda como está ahora).",
        )
    )
}

fn config_read_error(path: &Path, error: &io::Error, locale: Locale) -> String {
    if error.kind() == io::ErrorKind::InvalidData {
        format!(
            "{} {}. {}",
            locale.pick(
                "O config.ini do perfil não é texto UTF-8:",
                "The profile’s config.ini is not UTF-8 text:",
                "El config.ini del perfil no es texto UTF-8:",
            ),
            path.display(),
            locale.pick(
                "Ele não foi alterado; salve-o como UTF-8 para usar o motor de desempenho.",
                "It was not changed; save it as UTF-8 to use the performance engine.",
                "No se ha modificado; guárdalo como UTF-8 para usar el motor de rendimiento.",
            )
        )
    } else {
        format!(
            "{} {} ({error}).",
            locale.pick(
                "Não foi possível ler o config.ini do perfil:",
                "Could not read the profile’s config.ini:",
                "No se ha podido leer el config.ini del perfil:",
            ),
            path.display()
        )
    }
}

fn write_error(path: &Path, error: &io::Error, locale: Locale) -> String {
    format!(
        "{} {} ({error}). {}",
        locale.pick(
            "Não foi possível gravar",
            "Could not write",
            "No se ha podido escribir",
        ),
        path.display(),
        locale.pick(
            "Verifique se o arquivo e a pasta não são somente leitura e se nenhum outro programa está com o arquivo aberto.",
            "Check that the file and its folder are not read-only and that no other program has the file open.",
            "Comprueba que el archivo y su carpeta no sean de solo lectura y que ningún otro programa tenga el archivo abierto.",
        )
    )
}

fn read_only_error(path: &Path, locale: Locale) -> String {
    format!(
        "{} {}. {}",
        locale.pick(
            "O config.ini do perfil é somente leitura:",
            "The profile’s config.ini is read-only:",
            "El config.ini del perfil es de solo lectura:",
        ),
        path.display(),
        locale.pick(
            "Ele não foi substituído. Desmarque o atributo somente leitura para que o motor de desempenho possa ajustá-lo e restaurá-lo.",
            "It was not replaced. Clear its read-only attribute so the performance engine can adjust and restore it.",
            "No se ha sustituido. Quita el atributo de solo lectura para que el motor de rendimiento pueda ajustarlo y restaurarlo.",
        )
    )
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn write_backup(path: &Path, backup: &Backup, locale: Locale) -> Result<()> {
    let written = if backup.is_empty() {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    } else {
        write_atomic(
            path,
            &serde_json::to_vec_pretty(&json!({"schema": 1, "original_lines": backup}))?,
        )
    };
    Ok(written.map_err(|error| write_error(path, &error, locale))?)
}

/// The profile's own `kernel.parameters` tokens followed by ours (those it
/// does not already contain).
fn merged_kernel_parameters(original_line: Option<&str>) -> String {
    let own = original_line
        .and_then(|line| line.split_once('='))
        .map_or("", |(_, value)| value.trim());
    let mut tokens: Vec<&str> = own.split_whitespace().collect();
    for token in KERNEL_PARAMETERS.split(' ') {
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    tokens.join(" ")
}

/// Applies or removes the preset's keys in the AVD's `config.ini`. Other lines
/// keep their bytes. The original line (or its absence) is recorded before the
/// first change and restored when the setting is turned off. Profiles that
/// never used the preset are not read or written.
/// `kernel.parameters` keeps the profile's own tokens and appends ours; the
/// exact original line is restored when the option is turned off.
pub(crate) fn sync_avd_config(
    avd: &Path,
    desired: &[(&str, Option<&str>)],
    locale: Locale,
) -> Result<bool> {
    let backup_path = avd.join(BACKUP);
    let saved = match fs::read(&backup_path) {
        Ok(bytes) => {
            parse_backup(&bytes).map_err(|detail| backup_error(&backup_path, &detail, locale))?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if desired.iter().all(|(_, value)| value.is_none()) {
                return Ok(false);
            }
            Backup::new()
        }
        Err(error) => return Err(backup_error(&backup_path, &error.to_string(), locale).into()),
    };
    let config_path = avd.join("config.ini");
    let original = fs::read_to_string(&config_path)
        .map_err(|error| config_read_error(&config_path, &error, locale))?;
    let mut lines = split_lines(&original);
    let mut next = saved.clone();
    for &(key, value) in desired {
        match value {
            Some(value) => {
                let current = find_key(&lines, key);
                let recorded = next
                    .entry(key.to_owned())
                    .or_insert_with(|| current.map(|index| lines[index].text.clone()));
                let value = if key == KERNEL_KEY {
                    merged_kernel_parameters(recorded.as_deref())
                } else {
                    value.to_owned()
                };
                let value = value.as_str();
                let matches = current.is_some_and(|index| {
                    lines[index]
                        .text
                        .split_once('=')
                        .is_some_and(|(_, current)| current.trim() == value)
                });
                if !matches {
                    replace_key(&mut lines, key, format!("{key}={value}"));
                }
            }
            None => match next.remove(key) {
                Some(Some(previous)) => replace_key(&mut lines, key, previous),
                Some(None) => remove_key(&mut lines, key),
                None => {}
            },
        }
    }
    let updated: String = lines
        .iter()
        .flat_map(|line| [line.text.as_str(), line.end.as_str()])
        .collect();
    // Record every original before config.ini changes, drop entries afterwards.
    let mut recorded = saved.clone();
    for (key, value) in &next {
        recorded.entry(key.clone()).or_insert_with(|| value.clone());
    }
    let changed = updated != original;
    // A read-only config.ini is reported, not replaced: the rename used for
    // the atomic write would otherwise bypass the attribute's intent.
    if changed
        && fs::metadata(&config_path)
            .map_err(|error| config_read_error(&config_path, &error, locale))?
            .permissions()
            .readonly()
    {
        return Err(read_only_error(&config_path, locale).into());
    }
    if recorded != saved {
        write_backup(&backup_path, &recorded, locale)?;
    }
    if changed {
        write_atomic(&config_path, updated.as_bytes())
            .map_err(|error| write_error(&config_path, &error, locale))?;
    }
    if next != recorded {
        write_backup(&backup_path, &next, locale)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALES: [Locale; 3] = [Locale::PtBr, Locale::EnUs, Locale::EsEs];
    const HASH_A: &str = "7476c5d7cf249182b7310d45e5304778bd5a337e8d6d11890edb27954b0f9717";

    fn manifest_json(capabilities: &[&str]) -> Value {
        json!({
            "schema": 1,
            "kind": MANIFEST_KIND,
            "engine_version": "37.3.10",
            "build_id": "qa-whpx-16970787",
            "emulator_sha256": HASH_A,
            "qemu_sha256": "74FDA2BF1C61A019F628171944A573F033FC97DD88DCC5FFDA3E2C03BDCB23E5",
            "gfxstream_sha256": "d92f6060d3dc054c54000ba5231b62bbcbcd61d6e1c08820c98ea4f2cc8360d0",
            "capabilities": capabilities,
        })
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn envs(command: &Command) -> BTreeMap<String, Option<String>> {
        command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn manifest_parses_known_capabilities_and_rejects_incomplete_files() {
        let all = [
            "tsc_sync",
            "lapic_fastpath",
            "kvm_pv_eoi",
            "vulkan_cached_host",
            "vulkan_cached_device_local_v2",
            "asg_spin",
            "vulkan_native_swapchain",
            "vcpu_placement_high",
            "guest_kernel_parameters",
            "future_gate",
            "tsc_sync",
        ];
        // The old low-core placement capability no longer enables anything.
        let old = parse_manifest(
            &manifest_json(&["vcpu_placement"]).to_string(),
            Locale::EnUs,
        );
        assert!(old.unwrap().capabilities.is_empty());
        let manifest = parse_manifest(&manifest_json(&all).to_string(), Locale::EnUs).unwrap();
        assert_eq!(manifest.engine_version, "37.3.10");
        assert_eq!(manifest.build_id, "qa-whpx-16970787");
        assert_eq!(manifest.capabilities, Capability::ALL);
        assert_eq!(manifest.files.len(), 3);
        assert_eq!(manifest.files[1].0, QEMU);
        assert!(
            manifest.files[1].1.starts_with("74fda2bf"),
            "hashes are lowercased"
        );

        let mut extra = manifest_json(&[]);
        extra["extra_sha256"] = json!({"emulator/lib64/libemugl_common.dll": HASH_A});
        let parsed = parse_manifest(&extra.to_string(), Locale::EnUs).unwrap();
        assert_eq!(parsed.files.len(), 4);
        assert!(parsed.capabilities.is_empty());

        let mut broken = Vec::new();
        for field in [
            "schema",
            "kind",
            "engine_version",
            "build_id",
            "emulator_sha256",
            "qemu_sha256",
            "gfxstream_sha256",
            "capabilities",
        ] {
            let mut value = manifest_json(&["tsc_sync"]);
            value.as_object_mut().unwrap().remove(field);
            broken.push(value);
        }
        let mut short_hash = manifest_json(&[]);
        short_hash["qemu_sha256"] = json!("abc");
        broken.push(short_hash);
        let mut other_kind = manifest_json(&[]);
        other_kind["kind"] = json!("cache");
        broken.push(other_kind);
        for path in [
            "../emulator.exe",
            "emulator/../../x.dll",
            "emulator\\x.dll",
            "C:/x.dll",
            "emulator/emulator.exe",
            "sdk/x.dll",
        ] {
            let mut value = manifest_json(&[]);
            value["extra_sha256"] = json!({ (path): HASH_A });
            broken.push(value);
        }
        for value in broken {
            for locale in LOCALES {
                assert!(
                    parse_manifest(&value.to_string(), locale).is_err(),
                    "{value}"
                );
            }
        }
        assert!(parse_manifest("not json", Locale::PtBr).is_err());
    }

    #[test]
    fn manifest_is_found_from_the_stage_or_its_sdk_folder_and_hashes_are_checked() {
        let stage = tempfile::tempdir().unwrap();
        let sdk = stage.path().join("sdk");
        for (path, bytes) in [(EMULATOR, "launcher"), (QEMU, "qemu"), (GFXSTREAM, "gfx")] {
            let path = sdk.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        assert!(read_manifest(stage.path(), Locale::EnUs).is_err());
        assert!(manifest_capabilities(stage.path()).is_none());

        let mut value = manifest_json(&["asg_spin"]);
        value["emulator_sha256"] = json!(sha256_file(&sdk.join(EMULATOR)).unwrap());
        value["qemu_sha256"] = json!(sha256_file(&sdk.join(QEMU)).unwrap());
        value["gfxstream_sha256"] = json!(sha256_file(&sdk.join(GFXSTREAM)).unwrap());
        fs::write(stage.path().join(MANIFEST), value.to_string()).unwrap();
        for path in [stage.path().to_owned(), sdk.clone()] {
            let (found, manifest) = read_manifest(&path, Locale::EnUs).unwrap();
            assert_eq!(found, stage.path());
            verify(&sdk, &manifest, Locale::EnUs).unwrap();
        }
        assert_eq!(manifest_capabilities(&sdk), Some(vec![Capability::AsgSpin]));
        assert!(read_manifest(&stage.path().join("other"), Locale::EnUs).is_err());

        // A changed module or a missing file is refused.
        fs::write(sdk.join(GFXSTREAM), "patched again").unwrap();
        let (_, manifest) = read_manifest(stage.path(), Locale::EnUs).unwrap();
        let error = verify(&sdk, &manifest, Locale::EnUs)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("SHA-256") && error.contains("libgfxstream_backend.dll"),
            "{error}"
        );
        fs::remove_file(sdk.join(QEMU)).unwrap();
        assert!(verify(&sdk, &manifest, Locale::EnUs).is_err());
    }

    #[test]
    fn a_missing_manifest_and_an_unreadable_one_give_different_reasons() {
        let stage = tempfile::tempdir().unwrap();
        let missing = read_manifest(stage.path(), Locale::EnUs)
            .unwrap_err()
            .to_string();
        assert_eq!(
            missing,
            format!("Choose the engine folder containing {MANIFEST}")
        );
        #[cfg(windows)]
        for deny_folder in [false, true] {
            use super::super::engine_folder::DenyRead;
            let stage = tempfile::tempdir().unwrap();
            fs::write(stage.path().join(MANIFEST), manifest_json(&[]).to_string()).unwrap();
            let error = {
                let _deny = DenyRead::new(&if deny_folder {
                    stage.path().to_owned()
                } else {
                    stage.path().join(MANIFEST)
                });
                read_manifest(stage.path(), Locale::PtBr)
                    .unwrap_err()
                    .to_string()
            };
            assert!(error.starts_with("Sem permissão para ler"), "{error}");
            assert!(
                error.contains(&stage.path().display().to_string()),
                "{error}"
            );
            // Readable again once the ACL is restored.
            assert!(manifest_capabilities(stage.path()).is_some());
        }
    }

    #[test]
    fn selection_requires_a_windows_sdk_image_and_host_gpu() {
        let dir = tempfile::tempdir().unwrap();
        for (sdk_image, gpu) in [
            (false, Gpu::Host),
            (true, Gpu::Software),
            (true, Gpu::SwiftShader),
        ] {
            for locale in LOCALES {
                assert!(select(dir.path(), sdk_image, gpu, locale).is_err());
            }
        }
        assert!(
            fallback_message(Locale::EnUs, "x")
                .starts_with("The performance engine failed verification")
        );
    }

    #[test]
    fn default_preset_is_conservative_and_only_uses_listed_capabilities() {
        let defaults = PerformanceOptions::default();
        assert!(!defaults.guest_kernel_parameters);
        let everything = Capability::ALL.to_vec();
        let plan = Plan::new(&defaults, &everything, true);
        assert_eq!(plan.tunings, Tuning::ALL[..7]);
        assert!(plan.held_back.is_empty());

        let mut command = Command::new("emulator");
        command.env("RUSTEMUDROID_QA_PIPE_LOCKLESS", "1");
        command.env("RUSTEMUDROID_QA_VCPU_CORES", "0,1,2,3");
        plan.apply(&mut command);
        let env = envs(&command);
        let set = |key: &str| env.get(key).cloned().flatten();
        for (key, value) in [
            ("RUSTEMUDROID_QA_WHPX_SYNC_TSC", "1"),
            ("RUSTEMUDROID_QA_LAPIC_FASTPATH", "1"),
            ("RUSTEMUDROID_QA_KVM_PV_EOI", "1"),
            ("RUSTEMUDROID_QA_VULKAN_CACHED_HOST", "1"),
            ("RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL", "2"),
            ("RUSTEMUDROID_QA_ASG_SPIN_US", "3000"),
            ("RUSTEMUDROID_QA_VCPU_PLACEMENT", "sets-high"),
        ] {
            assert_eq!(set(key).as_deref(), Some(value), "{key}");
        }
        // Inherited gates are cleared, not passed through.
        assert_eq!(env.get("RUSTEMUDROID_QA_PIPE_LOCKLESS"), Some(&None));
        assert_eq!(env.get("RUSTEMUDROID_QA_VCPU_CORES"), Some(&None));
        assert!(
            args(&command).is_empty(),
            "features go into one -feature argument"
        );
        assert_eq!(plan.features(), ["VulkanNativeSwapchain"]);
        assert_eq!(
            avd_settings(Some(&plan)),
            [(GLTRANSPORT_KEY, Some("asg")), (KERNEL_KEY, None)]
        );

        // Each option on its own.
        let none = PerformanceOptions {
            clock_sync: false,
            lapic_fastpath: false,
            kvm_pv_eoi: false,
            vulkan_memory_cache: false,
            asg_transport: false,
            vulkan_composition: false,
            dedicated_vcpu_cores: false,
            guest_kernel_parameters: false,
        };
        for tuning in Tuning::ALL {
            let mut options = none.clone();
            *tuning.enabled_mut(&mut options) = true;
            let plan = Plan::new(&options, &everything, true);
            assert_eq!(plan.tunings, [tuning]);
            let mut command = Command::new("emulator");
            plan.apply(&mut command);
            let set: Vec<_> = envs(&command)
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| format!("{key}={value}")))
                .collect();
            let expected: &[&str] = match tuning {
                Tuning::ClockSync => &["RUSTEMUDROID_QA_WHPX_SYNC_TSC=1"],
                Tuning::LapicFastpath => &["RUSTEMUDROID_QA_LAPIC_FASTPATH=1"],
                Tuning::KvmPvEoi => &["RUSTEMUDROID_QA_KVM_PV_EOI=1"],
                Tuning::VulkanMemoryCache => &[
                    "RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2",
                    "RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1",
                ],
                Tuning::AsgTransport => &["RUSTEMUDROID_QA_ASG_SPIN_US=3000"],
                Tuning::DedicatedVcpuCores => &["RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high"],
                Tuning::VulkanComposition | Tuning::GuestKernelParameters => &[],
            };
            assert_eq!(set, expected, "{tuning:?}");
            assert!(args(&command).is_empty());
            assert_eq!(
                plan.features().is_empty(),
                tuning != Tuning::VulkanComposition
            );
            let keys = avd_settings(Some(&plan));
            assert_eq!(keys[0].1.is_some(), tuning == Tuning::AsgTransport);
            assert_eq!(
                keys[1].1,
                (tuning == Tuning::GuestKernelParameters).then_some(KERNEL_PARAMETERS)
            );
            // The same option is dropped when the engine does not list it.
            let without: Vec<_> = everything
                .iter()
                .copied()
                .filter(|c| *c != tuning.capability())
                .collect();
            assert!(Plan::new(&options, &without, true).tunings.is_empty());
        }

        // Too few cores: placement is held back and reported, the rest stays.
        let small = Plan::new(&defaults, &everything, false);
        assert_eq!(small.held_back, [Tuning::DedicatedVcpuCores]);
        assert!(!small.tunings.contains(&Tuning::DedicatedVcpuCores));
        let mut command = Command::new("emulator");
        small.apply(&mut command);
        assert_eq!(envs(&command)["RUSTEMUDROID_QA_VCPU_PLACEMENT"], None);
        let status = small.status(&[], "", Locale::EnUs, Path::new("boot.log"));
        assert!(
            status.ends_with(
                "Not used on this computer (not enough physical cores): dedicated cores."
            ),
            "{status}"
        );

        // Host cache without the v2 device-local gate.
        let plan = Plan::new(&defaults, &[Capability::VulkanCachedHost], true);
        let mut command = Command::new("emulator");
        plan.apply(&mut command);
        let env = envs(&command);
        assert_eq!(env["RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL"], None);
        assert_eq!(
            env["RUSTEMUDROID_QA_VULKAN_CACHED_HOST"].as_deref(),
            Some("1")
        );
        assert_eq!(
            avd_settings(None),
            [(GLTRANSPORT_KEY, None), (KERNEL_KEY, None)]
        );
    }

    #[test]
    fn placement_is_offered_only_with_spare_physical_cores() {
        assert!(placement_fits(Some(8), 4));
        assert!(!placement_fits(Some(6), 4));
        assert!(!placement_fits(Some(4), 4));
        assert!(!placement_fits(None, 4));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(avd_vcpus(dir.path()), 4);
        fs::write(dir.path().join("config.ini"), "AvdId=x\nhw.cpu.ncore = 6\n").unwrap();
        assert_eq!(avd_vcpus(dir.path()), 6);
        if cfg!(windows) {
            assert!(physical_cores().is_some_and(|cores| cores >= 1));
        }
    }

    #[test]
    fn every_inherited_qa_variable_is_cleared() {
        let names = [
            "RUSTEMUDROID_QA_PIPE_LOCKLESS",
            "RUSTEMUDROID_QA_SOMETHING_NEWER",
            "rustemudroid_qa_lowercase",
            "RUSTEMUDROID_QA",
            "RUSTEMUDROID_LANG",
            "PATH",
        ]
        .map(OsString::from);
        assert_eq!(
            inherited_qa_gates(names.into_iter()),
            [
                "RUSTEMUDROID_QA_PIPE_LOCKLESS",
                "RUSTEMUDROID_QA_SOMETHING_NEWER",
                "rustemudroid_qa_lowercase"
            ]
            .map(OsString::from)
        );
        assert!(QA_GATES.iter().all(|gate| gate.starts_with(QA_PREFIX)));
    }

    #[test]
    fn clock_proof_needs_init_or_reset() {
        for line in [
            "qemu: info: WHPX TSC QA: synchronized init vcpus=4",
            "qemu: info: WHPX TSC QA: synchronized reset vcpus=4",
            "WHPX TSC QA: synchronized init",
        ] {
            assert!(clock_confirmed(line), "{line}");
        }
        for line in [
            "WHPX TSC QA: synchronized",
            "WHPX TSC QA: synchronized initially failed",
            "WHPX TSC QA: synchronizedinit",
            "WHPX TSC QA: not synchronized",
            "WHPX TSC QA: synchronization off",
        ] {
            assert!(!clock_confirmed(line), "{line}");
        }
    }

    #[test]
    fn confirmation_never_waits_and_retries_the_guest_cmdline() {
        let mut options = PerformanceOptions::default();
        options.guest_kernel_parameters = true;
        let plan = Plan::new(&options, &Capability::ALL, true);
        let start = Instant::now();
        let cmdline = format!("console=ttynull {KERNEL_PARAMETERS}");
        let mut reads = 0;
        let guest = |answer: bool| {
            let cmdline = cmdline.clone();
            move || answer.then(|| cmdline.clone())
        };

        // Everything confirmed at boot completion: final at once.
        let mut confirmation = Confirmation::new(start);
        let mut read = guest(true);
        assert_eq!(
            confirmation.check(&plan, start, &full_log(), &mut read, false),
            Some(vec![])
        );

        // A buffered engine: not final until the window closes or the VM stops.
        let early = full_log().replace("synchronized init", "x");
        let mut confirmation = Confirmation::new(start);
        let mut failing = || {
            reads += 1;
            None::<String>
        };
        for tick in 0..8 {
            let now = start + Duration::from_millis(500 * tick);
            assert_eq!(
                confirmation.check(&plan, now, &early, &mut failing, false),
                None
            );
        }
        assert_eq!(reads, usize::from(CMDLINE_TRIES), "bounded guest reads");
        let mut read = guest(true);
        let late = start + CONFIRM_WINDOW;
        assert_eq!(
            confirmation.check(&plan, late, &early, &mut read, false),
            Some(vec![Tuning::ClockSync, Tuning::GuestKernelParameters])
        );

        // The cmdline arrives on a later try, and the clock line at exit.
        let mut confirmation = Confirmation::new(start);
        let mut none = guest(false);
        assert_eq!(
            confirmation.check(&plan, start, &early, &mut none, false),
            None
        );
        let mut read = guest(true);
        assert_eq!(
            confirmation.check(&plan, start, &early, &mut read, false),
            None
        );
        assert_eq!(
            confirmation.check(&plan, start, &early, &mut none, true),
            Some(vec![Tuning::ClockSync])
        );
        assert_eq!(
            confirmation.check(&plan, start, &full_log(), &mut none, true),
            Some(vec![])
        );
    }

    const PLACEMENT_LINE: &str = "qemu-system-x86_64.exe: info: RustEmuDroid QA: vCPU placement mode=sets-high vcpu->cpus: 0->{8,9} 1->{10,11} 2->{12,13} 3->{14,15} default={0,1,2,3,4,5,6,7}";

    fn full_log() -> String {
        [
            "INFO         |     useVulkanNativeSwapchain: true",
            "INFO         | RustEmuDroid QA: requested cached Vulkan host allocations",
            "WARNING      | RustEmuDroid QA: cached mappable device-local Vulkan allocations active (v2), guest->host: 5->3",
            "INFO         |   androidboot.hardware.gltransport=asg",
            "WARNING      | RustEmuDroid QA: ASG consumer spin active, spin_us=3000",
            "qemu-system-x86_64.exe: info: RustEmuDroid QA: LAPIC MMIO fast path active (32-bit MOV)",
            "qemu-system-x86_64.exe: info: RustEmuDroid QA: KVM PV EOI active",
            PLACEMENT_LINE,
            "qemu-system-x86_64.exe: info: WHPX TSC QA: synchronized init vcpus=4",
        ]
        .join("\n")
    }

    #[test]
    fn startup_status_reports_options_without_their_proof_line() {
        let mut options = PerformanceOptions::default();
        options.guest_kernel_parameters = true;
        let plan = Plan::new(&options, &Capability::ALL, true);
        let log = full_log();
        let cmdline = format!("console=ttynull {KERNEL_PARAMETERS} ramoops.mem_address=0xff018000");
        assert!(plan.inactive(&log, Some(&cmdline)).is_empty());
        assert_eq!(plan.inactive(&log, None), [Tuning::GuestKernelParameters]);
        assert_eq!(
            plan.inactive(&log, Some("mitigations=off init_on_alloc=0")),
            [Tuning::GuestKernelParameters]
        );
        assert_eq!(plan.inactive("", Some(&cmdline)), Tuning::ALL[..7]);
        for (proof, tuning) in [
            ("LAPIC MMIO fast path active", Tuning::LapicFastpath),
            ("KVM PV EOI active", Tuning::KvmPvEoi),
            ("WHPX TSC QA", Tuning::ClockSync),
            ("synchronized init", Tuning::ClockSync),
        ] {
            let missing = log.replace(proof, "x");
            assert_eq!(plan.inactive(&missing, Some(&cmdline)), [tuning]);
        }
        let host_only = log.replace("(v2)", "");
        assert_eq!(
            plan.inactive(&host_only, Some(&cmdline)),
            [Tuning::VulkanMemoryCache]
        );

        // Placement: the mode token must be sets-high and a mapping must follow.
        let other_numbers = log.replace("0->{8,9} 1->{10,11}", "0->{4,5} 1->{6,7}");
        assert!(plan.inactive(&other_numbers, Some(&cmdline)).is_empty());
        for broken in [
            log.replace("mode=sets-high", "mode=sets"),
            log.replace("mode=sets-high", "mode=ideal"),
            log.replace(" vcpu->cpus: 0->{8,9}", " vcpu->cpus:"),
            log.replace(PLACEMENT_LINE, ""),
        ] {
            assert_eq!(
                plan.inactive(&broken, Some(&cmdline)),
                [Tuning::DedicatedVcpuCores]
            );
        }
        let fallback = format!(
            "{}\nqemu: warning: RustEmuDroid QA: vCPU placement 'sets' needs CPU sets and >= 6 cores (have 4), using 'ideal'",
            log.replace("mode=sets-high", "mode=ideal")
        );
        let inactive = plan.inactive(&fallback, Some(&cmdline));
        let path = Path::new("boot.log");
        let status = plan.status(&inactive, &fallback, Locale::EnUs, path);
        assert!(
            status.ends_with(
                "See boot.log Engine: vCPU placement 'sets' needs CPU sets and >= 6 cores (have 4), using 'ideal'"
            ),
            "{status}"
        );

        assert_eq!(
            plan.status(&[], &log, Locale::EnUs, path),
            "Performance engine active: synchronized clock, fast APIC access, paravirtualized end of interrupt, Vulkan memory cache, shared graphics transport, Vulkan composition, dedicated cores, kernel parameters."
        );
        let partial = plan.status(&[Tuning::DedicatedVcpuCores], &log, Locale::EnUs, path);
        assert!(partial.starts_with(
            "Not confirmed in the engine log (this does not mean it is off; older engines write some lines only at exit): dedicated cores. Confirmed: synchronized clock"
        ));
        assert!(partial.ends_with("See boot.log"));
        for locale in LOCALES {
            let all = plan.status(&Tuning::ALL, "", locale, path);
            assert!(all.contains(" -. "), "{all}");
            assert!(
                !Plan::new(&options, &[], true)
                    .status(&[], "", locale, path)
                    .is_empty()
            );
        }
    }

    fn sync(avd: &Path, gl: Option<&str>, kernel: Option<&str>) -> bool {
        sync_avd_config(
            avd,
            &[(GLTRANSPORT_KEY, gl), (KERNEL_KEY, kernel)],
            Locale::EnUs,
        )
        .unwrap()
    }

    #[test]
    fn avd_keys_are_applied_idempotently_and_removed_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.ini");
        let backup = dir.path().join(BACKUP);
        for original in [
            "AvdId=RustEmuDroid\nhw.cpu.ncore=4\nimage.sysdir.1=system-images/android-37.0/google_apis_playstore/x86_64/\n",
            "AvdId = RustEmuDroid\r\nhw.gltransport = pipe\r\nhw.ramSize = 8192\r\n",
            "AvdId=RustEmuDroid\nkernel.parameters=quiet\nPlayStore.enabled=true",
            "hw.lcd.vsync=60",
            "",
        ] {
            fs::write(&config, original).unwrap();
            // A profile that never used the preset is not touched.
            assert!(!sync(dir.path(), None, None));
            assert!(!backup.exists());

            assert!(sync(dir.path(), Some("asg"), Some(KERNEL_PARAMETERS)));
            let applied = fs::read_to_string(&config).unwrap();
            assert_eq!(
                super::super::property(&applied, GLTRANSPORT_KEY),
                Some("asg")
            );
            // The profile's own kernel.parameters tokens are kept.
            let expected = if original.contains("kernel.parameters=quiet") {
                format!("quiet {KERNEL_PARAMETERS}")
            } else {
                KERNEL_PARAMETERS.to_owned()
            };
            assert_eq!(
                super::super::property(&applied, KERNEL_KEY),
                Some(expected.as_str())
            );
            for line in original
                .lines()
                .filter(|l| !l.contains("gltransport") && !l.contains("kernel."))
            {
                assert!(applied.contains(line), "{line:?} kept");
            }
            assert!(backup.is_file());
            assert!(
                !sync(dir.path(), Some("asg"), Some(KERNEL_PARAMETERS)),
                "idempotent"
            );
            assert_eq!(fs::read_to_string(&config).unwrap(), applied);

            // Turning one option off restores only that key.
            assert!(sync(dir.path(), Some("asg"), None));
            let partial = fs::read_to_string(&config).unwrap();
            assert_eq!(
                super::super::property(&partial, KERNEL_KEY),
                super::super::property(original, KERNEL_KEY)
            );
            assert!(sync(dir.path(), None, None));
            assert_eq!(
                fs::read(&config).unwrap(),
                original.as_bytes(),
                "{original:?}"
            );
            assert!(!backup.exists());
            assert!(!sync(dir.path(), None, None));
        }
    }

    #[test]
    fn avd_restore_survives_the_emulator_rewriting_config_ini() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.ini");
        fs::write(&config, "AvdId=RustEmuDroid\nhw.gltransport=pipe\n").unwrap();
        assert!(sync(dir.path(), Some("asg"), None));
        // The emulator may reformat the file while the preset is on.
        fs::write(
            &config,
            "AvdId = RustEmuDroid\nhw.gltransport = asg\nhw.new = 1\n",
        )
        .unwrap();
        assert!(!sync(dir.path(), Some("asg"), None));
        assert!(sync(dir.path(), None, None));
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            "AvdId = RustEmuDroid\nhw.gltransport=pipe\nhw.new = 1\n"
        );
        // A user value present before the first change is restored, not deleted.
        fs::write(&config, "hw.gltransport=asg\n").unwrap();
        assert!(!sync(dir.path(), Some("asg"), None));
        assert!(!sync(dir.path(), None, None));
        assert_eq!(fs::read_to_string(&config).unwrap(), "hw.gltransport=asg\n");
        assert!(!dir.path().join(BACKUP).exists());
        // An unreadable record is reported instead of guessed.
        fs::write(dir.path().join(BACKUP), "{").unwrap();
        assert!(sync_avd_config(dir.path(), &[(GLTRANSPORT_KEY, None)], Locale::EnUs).is_err());
    }

    #[test]
    fn kernel_parameters_keep_the_profile_tokens() {
        assert_eq!(merged_kernel_parameters(None), KERNEL_PARAMETERS);
        assert_eq!(
            merged_kernel_parameters(Some("kernel.parameters =  ")),
            KERNEL_PARAMETERS
        );
        assert_eq!(
            merged_kernel_parameters(Some("kernel.parameters = quiet init_on_alloc=0")),
            "quiet init_on_alloc=0 mitigations=off transparent_hugepage=always"
        );
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.ini");
        let original = "AvdId=RustEmuDroid\r\nkernel.parameters = quiet loglevel=3\r\n";
        fs::write(&config, original).unwrap();
        assert!(sync(dir.path(), None, Some(KERNEL_PARAMETERS)));
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            format!(
                "AvdId=RustEmuDroid\r\nkernel.parameters=quiet loglevel=3 {KERNEL_PARAMETERS}\r\n"
            )
        );
        assert!(
            !sync(dir.path(), None, Some(KERNEL_PARAMETERS)),
            "idempotent"
        );
        assert!(sync(dir.path(), None, None));
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
    }

    #[test]
    fn config_sync_errors_name_the_file_and_what_to_do() {
        let on = [(GLTRANSPORT_KEY, Some("asg"))];
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.ini");
        let backup = dir.path().join(BACKUP);

        fs::write(&config, "hw.gltransport=pipe\n").unwrap();
        for (bytes, detail) in [(&b"{"[..], "EOF"), (&br#"{"schema": 9}"#[..], "schema 9")] {
            fs::write(&backup, bytes).unwrap();
            for locale in LOCALES {
                let error = sync_avd_config(dir.path(), &on, locale)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(&backup.display().to_string()), "{error}");
                assert!(error.contains("rustemudroid-performance.json"), "{error}");
            }
            let error = sync_avd_config(dir.path(), &on, Locale::EnUs)
                .unwrap_err()
                .to_string();
            assert!(error.contains(detail), "{error}");
            assert!(error.contains("Deleting rustemudroid-performance.json discards"));
        }
        fs::remove_file(&backup).unwrap();

        fs::write(&config, b"hw.gltransport=pipe\n\xff\n").unwrap();
        let error = sync_avd_config(dir.path(), &on, Locale::EnUs)
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("The profile’s config.ini is not UTF-8 text:")
                && error.contains(&config.display().to_string()),
            "{error}"
        );
        assert!(!backup.exists());

        // Read-only: reported, not replaced, and no record is left behind.
        fs::write(&config, "hw.gltransport=pipe\n").unwrap();
        let mut permissions = fs::metadata(&config).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&config, permissions.clone()).unwrap();
        let error = sync_avd_config(dir.path(), &on, Locale::PtBr)
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("O config.ini do perfil é somente leitura:"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            "hw.gltransport=pipe\n"
        );
        assert!(fs::metadata(&config).unwrap().permissions().readonly());
        assert!(!backup.exists());
        // Restored so the temporary folder can be removed.
        permissions.set_readonly(false);
        fs::set_permissions(&config, permissions).unwrap();
    }
}
