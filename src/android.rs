use std::{
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf, absolute},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{Engine, Gpu, Result, locale::Locale};

const AVD: &str = "RustEmuDroid";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const KEMU_EMULATOR: &str = "qemu/windows-x86_64/qemu-system-x86_64.exe";
const KEMU_IMAGE: &str = "system-img";

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeKind {
    Sdk,
    Portable,
    Kemu,
}

pub struct Sdk {
    pub root: PathBuf,
    pub adb: PathBuf,
    emulator: PathBuf,
    locale: Locale,
    kind: RuntimeKind,
}

impl Sdk {
    #[cfg(feature = "ui")]
    pub fn find(explicit: Option<PathBuf>) -> Result<Self> {
        Self::find_localized(explicit, Locale::default())
    }

    pub fn find_localized(explicit: Option<PathBuf>, locale: Locale) -> Result<Self> {
        let configured = explicit
            .or_else(|| env::var_os("ANDROID_HOME").map(PathBuf::from))
            .or_else(|| env::var_os("ANDROID_SDK_ROOT").map(PathBuf::from));
        let portable = env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|parent| parent.join("runtime")));
        let managed = crate::runtime_install::default_destination().ok();
        if let (Some(selected), Some(root)) = (&configured, &managed)
            && same_directory(selected, root)
        {
            return Self::from_root(root.clone(), RuntimeKind::Portable, locale);
        }
        let allow_fallback = configured.is_none();
        let found = Self::discover(
            configured,
            portable,
            default_sdk_path(),
            cfg!(windows).then(kemu_install_root).flatten(),
            locale,
        );
        found.or_else(|error| {
            if allow_fallback
                && let Some(root) = managed
                && root.is_dir()
            {
                Self::from_root(root, RuntimeKind::Portable, locale)
            } else {
                Err(error)
            }
        })
    }

    fn discover(
        configured: Option<PathBuf>,
        portable: Option<PathBuf>,
        default: Option<PathBuf>,
        kemu: Option<PathBuf>,
        locale: Locale,
    ) -> Result<Self> {
        if let Some(root) = configured {
            let kind = if portable.is_some_and(|path| same_directory(&root, &path)) {
                RuntimeKind::Portable
            } else {
                RuntimeKind::Sdk
            };
            return Self::from_root(root, kind, locale);
        }
        if let Some(root) = portable
            && let Ok(runtime) = Self::from_root(root, RuntimeKind::Portable, locale)
        {
            return Ok(runtime);
        }
        let sdk = default
            .filter(|root| root.exists())
            .ok_or_else(|| -> Box<dyn std::error::Error> {
                let command = if cfg!(windows) {
                    r".\rustemudroid.exe runtime-setup --image google-play"
                } else {
                    "./rustemudroid runtime-setup --image google-play"
                };
                let message = locale.pick(
                    "Runtime Android não encontrado. Execute o comando abaixo na pasta do programa para consultar os detalhes da instalação.",
                    "Android runtime not found. Run the following command from the extracted application folder to review the installation.",
                    "No se ha encontrado el entorno de Android. Ejecuta el siguiente comando desde la carpeta del programa para consultar los detalles de instalación.",
                );
                format!("{message}\n{command}").into()
            })
            .and_then(|root| Self::from_root(root, RuntimeKind::Sdk, locale));
        if sdk.is_ok() {
            return sdk;
        }
        if let Some(root) = kemu
            && (root.join(KEMU_EMULATOR).is_file() || root.join("Kemu.exe").is_file())
        {
            return Self::from_root(root, RuntimeKind::Kemu, locale);
        }
        sdk
    }

    fn from_root(root: PathBuf, kind: RuntimeKind, locale: Locale) -> Result<Self> {
        let root = absolute(root)?;
        let (emulator, adb) = if kind == RuntimeKind::Kemu {
            (
                root.join(KEMU_EMULATOR),
                root.join("platform-tools/adb.exe"),
            )
        } else {
            let suffix = env::consts::EXE_SUFFIX;
            (
                root.join("emulator").join(format!("emulator{suffix}")),
                root.join("platform-tools").join(format!("adb{suffix}")),
            )
        };
        for tool in [&emulator, &adb] {
            if !tool.is_file() {
                return Err(format!(
                    "{} {}",
                    locale.pick(
                        "Ferramenta Android ausente em",
                        "Android tool missing at",
                        "Falta la herramienta de Android en",
                    ),
                    tool.display()
                )
                .into());
            }
        }
        if kind == RuntimeKind::Kemu {
            let image = root.join(KEMU_IMAGE);
            SystemImage::from_dir_localized(image.clone(), locale)?;
            if !is_kemu_image(&image) {
                return Err(locale
                    .pick(
                        "A pasta system-img não identifica uma imagem do Kemu.",
                        "The system-img directory does not identify a Kemu image.",
                        "La carpeta system-img no identifica una imagen de Kemu.",
                    )
                    .into());
            }
        }
        Ok(Self {
            root,
            emulator,
            adb,
            locale,
            kind,
        })
    }

    pub fn is_standalone_kemu(&self) -> bool {
        self.kind == RuntimeKind::Kemu
    }

    pub fn is_portable(&self) -> bool {
        self.kind == RuntimeKind::Portable
    }

    pub fn engine_path(&self) -> &Path {
        &self.emulator
    }

    fn require_sdk(&self) -> Result<()> {
        if self.is_standalone_kemu() {
            return Err(self
                .locale
                .pick(
                    "Esse modo exige o Android SDK. Informe o caminho com --sdk.",
                    "This mode requires the Android SDK. Set its path with --sdk.",
                    "Este modo requiere el SDK de Android. Indica su ruta con --sdk.",
                )
                .into());
        }
        Ok(())
    }

    pub fn require_matching_image(&self, image: &Path) -> Result<()> {
        if self.is_standalone_kemu() && !matching_kemu_image(&self.root, image) {
            return Err(self
                .locale
                .pick(
                    "Sem o Android SDK, use a imagem system-img da instalação do Kemu.",
                    "With only Kemu installed, use that installation's system-img image.",
                    "Si solo está instalado Kemu, usa la imagen system-img de esa instalación.",
                )
                .into());
        }
        Ok(())
    }

    fn validate_boot_mode(&self, engine: Engine, gpu: Gpu) -> Result<()> {
        if engine == Engine::Sdk || !matches!(gpu, Gpu::Host | Gpu::Auto) {
            self.require_sdk()?;
        }
        Ok(())
    }

    fn matching_runtime(
        &self,
        system: &Path,
        engine: Engine,
        gpu: Gpu,
        installed_kemu: Option<PathBuf>,
    ) -> Result<Option<PathBuf>> {
        self.validate_boot_mode(engine, gpu)?;
        if self.is_standalone_kemu() {
            self.require_matching_image(system)?;
            return Ok(Some(self.root.clone()));
        }
        Ok(installed_kemu.filter(|root| {
            engine == Engine::Auto
                && matches!(gpu, Gpu::Host | Gpu::Auto)
                && matching_kemu_image(root, system)
        }))
    }

    pub fn check_acceleration(&self) -> Result<()> {
        #[cfg(windows)]
        if self.is_standalone_kemu() {
            check_whpx(self.locale)?;
            println!(
                "{}",
                self.locale.pick(
                    "Hipervisor Windows disponível (WHPX).",
                    "Windows hypervisor available (WHPX).",
                    "Hipervisor de Windows disponible (WHPX).",
                )
            );
            return Ok(());
        }
        let status = self.sdk_acceleration_command().status()?;
        if !status.success() {
            return Err(format!(
                "{}: {status}",
                self.locale.pick(
                    "Falha na verificação do hipervisor",
                    "Hypervisor check failed",
                    "Error al comprobar el hipervisor"
                )
            )
            .into());
        }
        Ok(())
    }

    fn sdk_acceleration_command(&self) -> Command {
        #[cfg(windows)]
        {
            // The SDK launcher can split the helper's path on spaces when forwarding this check.
            let helper = self.emulator.with_file_name("emulator-check.exe");
            if helper.is_file() {
                let mut command = Command::new(helper);
                command.arg("accel");
                return command;
            }
        }
        let mut command = Command::new(&self.emulator);
        command.arg("-accel-check");
        command
    }
}

#[cfg(windows)]
fn check_whpx(locale: Locale) -> Result<()> {
    use windows_sys::{
        Win32::{
            Foundation::FreeLibrary,
            System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        },
        w,
    };

    type GetCapability = unsafe extern "system" fn(u32, *mut u64, u32, *mut u32) -> i32;
    // WHvCapabilityCodeHypervisorPresent returns BOOL in the capability buffer.
    let available = unsafe {
        let module = LoadLibraryExW(
            w!("WinHvPlatform.dll"),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if module.is_null() {
            false
        } else {
            let available = GetProcAddress(module, c"WHvGetCapability".as_ptr().cast())
                .is_some_and(|address| {
                    let capability = std::mem::transmute::<_, GetCapability>(address);
                    let mut value = 0u64;
                    let mut written = 0;
                    capability(0, &mut value, 8, &mut written) == 0
                        && written >= 4
                        && value as u32 != 0
                });
            FreeLibrary(module);
            available
        }
    };
    if !available {
        return Err(locale
            .pick(
                "O hipervisor Windows (WHPX) não está disponível para o Kemu.",
                "The Windows hypervisor (WHPX) is unavailable for Kemu.",
                "El hipervisor de Windows (WHPX) no está disponible para Kemu.",
            )
            .into());
    }
    Ok(())
}

fn same_directory(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left).is_ok_and(|path| fs::canonicalize(right).ok().as_ref() == Some(&path))
}

fn path_is_inside(path: &Path, root: &Path) -> bool {
    fs::canonicalize(root).is_ok_and(|root| {
        path.ancestors()
            .find_map(|ancestor| fs::canonicalize(ancestor).ok())
            .is_some_and(|parent| parent.starts_with(root))
    })
}

pub(crate) fn default_sdk_path() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(|home| PathBuf::from(home).join("Android/Sdk"))
    } else {
        env::var_os("HOME").map(|home| PathBuf::from(home).join("Android/Sdk"))
    }
}

pub fn kemu_install_root() -> Option<PathBuf> {
    env::var_os("RUSTEMUDROID_KEMU_ROOT")
        .map(PathBuf::from)
        .or_else(|| env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Kemu")))
}

pub struct Profile {
    pub root: PathBuf,
    locale: Locale,
    avd_home: PathBuf,
    user_home: PathBuf,
}

impl Profile {
    #[cfg(feature = "ui")]
    pub fn new(root: PathBuf) -> Self {
        Self::localized(root, Locale::default())
    }
    pub fn localized(root: PathBuf, locale: Locale) -> Self {
        Self {
            locale,
            avd_home: root.join("avd"),
            user_home: root.join("home"),
            root,
        }
    }

    pub fn init(&self, image: SystemImage) -> Result<()> {
        let gaming = image.tag.starts_with("google_apis_playstore");
        self.init_disk(image, gaming)
    }

    fn init_disk(&self, image: SystemImage, gaming: bool) -> Result<()> {
        self.ensure_writable_location()?;
        let SystemImage { api, tag, location } = image;
        let directory = self.avd_home.join(format!("{AVD}.avd"));
        let entry = self.avd_home.join(format!("{AVD}.ini"));
        let profile_path = directory
            .to_str()
            .filter(|path| !path.contains(['\r', '\n']))
            .ok_or("O diretório do AVD deve ser um caminho UTF-8 sem quebras de linha.")?;

        fs::create_dir_all(&directory)?;
        fs::create_dir_all(&self.user_home)?;
        let hardware = if gaming {
            "hw.ramSize=8192\nhw.lcd.width=1600\nhw.lcd.height=900\nhw.lcd.density=320\n\
             hw.initialOrientation=landscape\nhw.lcd.vsync=60\nuserdata.useQcow2=no\n\
             skin.dynamic=no\nskin.name=1600x900\nskin.path=1600x900\ndisk.dataPartition.size=32G"
        } else {
            "hw.ramSize=3072\nhw.lcd.width=1080\nhw.lcd.height=1920\nhw.lcd.density=420\ndisk.dataPartition.size=4G"
        };
        let play_store = tag.starts_with("google_apis_playstore");
        let config = format!(
            "AvdId={AVD}\nabi.type=x86_64\nhw.cpu.arch=x86_64\nhw.cpu.ncore=4\n\
             {hardware}\nhw.gpu.enabled=yes\nhw.gpu.mode=auto\nhw.keyboard=yes\nPlayStore.enabled={play_store}\n\
             image.sysdir.1={location}\ntag.id={tag}\n"
        );
        write_once(
            &directory.join("config.ini"),
            &config,
            &["image.sysdir.1", "tag.id", "abi.type"],
        )?;
        write_once(
            &entry,
            &format!("path={profile_path}\ntarget={api}\n"),
            &["path", "target"],
        )?;
        println!(
            "AVD {AVD} {} {}",
            self.locale.pick("pronto em", "ready at", "listo en"),
            directory.display()
        );
        Ok(())
    }

    fn ensure_writable_location(&self) -> Result<()> {
        if cfg!(windows)
            && let Some(root) = kemu_install_root()
            && root.join(KEMU_EMULATOR).is_file()
            && [
                self.root.clone(),
                self.root.join("vm-session.lock"),
                self.user_home.clone(),
                self.avd_home.join(format!("{AVD}.avd")),
                self.avd_home.join(format!("{AVD}.ini")),
            ]
            .iter()
            .any(|path| path_is_inside(path, &root))
        {
            return Err(self
                .locale
                .pick(
                    "Escolha um diretório de dados fora da instalação do Kemu.",
                    "Choose a data directory outside the Kemu installation.",
                    "Elige un directorio de datos fuera de la instalación de Kemu.",
                )
                .into());
        }
        Ok(())
    }

    pub(crate) fn system_dir(&self, sdk: &Sdk) -> Result<PathBuf> {
        let directory = self.avd_home.join(format!("{AVD}.avd"));
        let entry = self.avd_home.join(format!("{AVD}.ini"));
        let contents = fs::read_to_string(&entry)
            .map_err(|error| format!("Execute init. {error}: {}", entry.display()))?;
        let configured = property(&contents, "path");
        if configured.map(Path::new) != Some(directory.as_path())
            || !directory.join("config.ini").is_file()
        {
            return Err(format!(
                "O AVD de {} não aponta para os dados locais do protótipo.",
                entry.display()
            )
            .into());
        }
        let config = fs::read_to_string(directory.join("config.ini"))?;
        let location = property(&config, "image.sysdir.1")
            .ok_or("O perfil não informa o diretório da imagem Android.")?;
        let system_dir = sdk.root.join(location);
        if !system_dir.join("system.img").is_file() {
            return Err(format!("system.img ausente em {}", system_dir.display()).into());
        }
        Ok(system_dir)
    }
}

pub struct SystemImage {
    api: String,
    tag: String,
    location: String,
}

impl SystemImage {
    pub fn preferred(sdk: &Sdk) -> Result<Self> {
        if sdk.is_standalone_kemu() {
            return Self::from_dir_localized(sdk.root.join(KEMU_IMAGE), sdk.locale);
        }
        for package in [
            "system-images;android-37.0;google_apis_playstore;x86_64",
            "system-images;android-36;google_apis;x86_64",
            "system-images;android-35;google_apis;x86_64",
        ] {
            if let Ok(image) = Self::from_sdk(sdk, package) {
                return Ok(image);
            }
        }
        let images = sdk.root.join("system-images");
        let newest_aosp = fs::read_dir(images)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let api = entry.file_name().into_string().ok()?;
                let version = api.strip_prefix("android-")?;
                let (level, extension) = version.split_once('.').unwrap_or((version, "0"));
                let version = (level.parse::<u16>().ok()?, extension.parse::<u16>().ok()?);
                let package = format!("system-images;{api};default;x86_64");
                Self::from_sdk(sdk, &package)
                    .ok()
                    .map(|image| (version, image))
            })
            .max_by(|(left, a), (right, b)| {
                left.cmp(right).then_with(|| a.location.cmp(&b.location))
            });
        if let Some((_, image)) = newest_aosp {
            return Ok(image);
        }
        Err(sdk
            .locale
            .pick(
                "Instale a imagem pelo SDK Manager ou informe a pasta da imagem.",
                "Install a system image with SDK Manager or specify its directory.",
                "Instala una imagen con SDK Manager o indica su carpeta.",
            )
            .into())
    }
    pub fn from_sdk(sdk: &Sdk, package: &str) -> Result<Self> {
        sdk.require_sdk()?;
        let (api, tag) = parse_image(package).map_err(|_| {
            sdk.locale.pick(
                "Use um pacote system-images x86_64 válido.",
                "Use a valid x86_64 system-images package.",
                "Usa un paquete system-images x86_64 válido.",
            )
        })?;
        let location = format!("system-images/{api}/{tag}/x86_64/");
        if !sdk.root.join(&location).join("system.img").is_file() {
            return Err(format!(
                "{} {} ({package}, SDK Manager)",
                sdk.locale.pick(
                    "Imagem ausente em",
                    "Image missing at",
                    "Falta la imagen en"
                ),
                sdk.root.join(&location).display()
            )
            .into());
        }
        Ok(Self {
            api: api.into(),
            tag: tag.into(),
            location,
        })
    }

    pub fn from_dir_localized(path: PathBuf, locale: Locale) -> Result<Self> {
        let path = absolute(path)?;
        if !path.join("system.img").is_file() {
            return Err(format!(
                "{} {}",
                locale.pick(
                    "system.img ausente em",
                    "system.img missing at",
                    "Falta system.img en"
                ),
                path.display()
            )
            .into());
        }
        let metadata = fs::read_to_string(path.join("source.properties")).unwrap_or_default();
        let tag = property(&metadata, "SystemImage.TagId").unwrap_or("default");
        let props_path = path.join("build.prop");
        let props = fs::read_to_string(&props_path)
            .map_err(|error| format!("{}: {error}", props_path.display()))?;
        if property(&props, "ro.product.cpu.abi") != Some("x86_64") {
            return Err(format!(
                "{} {}",
                props_path.display(),
                locale.pick(
                    "deve declarar ABI x86_64.",
                    "must declare the x86_64 ABI.",
                    "debe declarar la ABI x86_64."
                )
            )
            .into());
        }
        let api: u16 = property(&props, "ro.build.version.sdk")
            .ok_or(locale.pick(
                "A imagem deve declarar ro.build.version.sdk.",
                "The image must declare ro.build.version.sdk.",
                "La imagen debe declarar ro.build.version.sdk.",
            ))?
            .parse()?;
        let location = path
            .to_str()
            .filter(|path| !path.contains(['\r', '\n']))
            .ok_or(locale.pick(
                "O caminho da imagem deve ser UTF-8 sem quebras de linha.",
                "The image path must be UTF-8 without line breaks.",
                "La ruta de la imagen debe ser UTF-8 sin saltos de línea.",
            ))?;
        Ok(Self {
            api: format!("android-{api}"),
            tag: tag.into(),
            location: location.into(),
        })
    }
}

fn property<'a>(contents: &'a str, name: &str) -> Option<&'a str> {
    contents.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == name).then_some(value.trim())
    })
}

fn parse_image(package: &str) -> Result<(&str, &str)> {
    let parts: Vec<_> = package.split(';').collect();
    if let ["system-images", api, tag, "x86_64"] = parts.as_slice() {
        let valid_api = api.strip_prefix("android-").is_some_and(|suffix| {
            suffix.split('.').count() <= 2
                && suffix
                    .split('.')
                    .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
        });
        if valid_api
            && matches!(
                *tag,
                "default" | "google_apis" | "google_apis_playstore" | "google_apis_playstore_ps16k"
            )
        {
            return Ok((api, tag));
        }
    }
    Err("Use system-images;android-<API>;google_apis;x86_64 ou a variante google_apis_playstore/default.".into())
}

fn write_once(path: &Path, content: &str, identity_keys: &[&str]) -> Result<()> {
    match OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(mut file) => file.write_all(content.as_bytes())?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(path)?;
            if !identity_keys
                .iter()
                .all(|key| property(&existing, key) == property(content, key))
            {
                return Err(format!(
                    "{} já contém outra configuração. Use outro --state-dir.",
                    path.display()
                )
                .into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn free_port_pair() -> Result<(u16, TcpListener, TcpListener)> {
    for port in (5554..=5584).step_by(2) {
        if let (Ok(console), Ok(adb)) = (
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)),
            TcpListener::bind((Ipv4Addr::LOCALHOST, port + 1)),
        ) {
            return Ok((port, console, adb));
        }
    }
    Err("Nenhum par de portas livres para console e ADB entre 5554 e 5585.".into())
}

#[derive(Default, Clone, Copy, PartialEq, clap::ValueEnum)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub enum Camera {
    #[default]
    Profile,
    EmulatedFront,
    WebcamFront,
    WebcamBack,
}

#[derive(Default, Clone, PartialEq, clap::Args)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Hardware {
    /// Nome do dispositivo Vulkan para os modos host e auto.
    #[arg(long = "gpu-device", default_value = "")]
    pub adapter: String,
    /// Origem da câmera do Android.
    #[arg(long, value_enum, default_value_t = Camera::Profile)]
    pub camera: Camera,
    /// Índice da webcam do computador para webcam-front ou webcam-back.
    #[arg(long, default_value_t = 0)]
    pub webcam_index: u8,
    /// Usar tradução ARM leve na imagem Kemu com Berberis (compatibilidade com PUBG).
    #[arg(long, default_value_t = false)]
    #[cfg_attr(feature = "ui", serde(default))]
    pub arm_lite: bool,
}

impl Hardware {
    fn apply(&self, command: &mut Command, gpu: Gpu) {
        if matches!(gpu, Gpu::Auto | Gpu::Host) && !self.adapter.is_empty() {
            command.env("ANDROID_EMU_VK_SELECT_GPU", &self.adapter);
        }
        match self.camera {
            Camera::Profile => {}
            Camera::EmulatedFront => {
                command.args(["-camera-front", "emulated"]);
            }
            Camera::WebcamFront | Camera::WebcamBack => {
                command
                    .arg(if self.camera == Camera::WebcamFront {
                        "-camera-front"
                    } else {
                        "-camera-back"
                    })
                    .arg(format!("webcam{}", self.webcam_index));
            }
        }
    }
}

pub struct BootOptions {
    pub gpu: Gpu,
    pub hardware: Hardware,
    pub amd_uma_vulkan: bool,
    pub engine: Engine,
    pub headless: bool,
    pub check: bool,
    pub embedded: bool,
    pub timeout: Duration,
}

pub enum BootEvent {
    Starting(String),
    Ready(String),
    Message(String),
    Display {
        console: u16,
        rpc: u16,
        avd: PathBuf,
        #[cfg(feature = "ui")]
        adb: PathBuf,
    },
}

impl std::fmt::Display for BootEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting(message) | Self::Ready(message) | Self::Message(message) => {
                f.write_str(message)
            }
            Self::Display {
                console, rpc, avd, ..
            } => write!(
                f,
                "Tela integrada em emulator-{console} (RPC {rpc}, {}).",
                avd.display()
            ),
        }
    }
}

pub(crate) fn is_kemu_image(system_dir: &Path) -> bool {
    fs::read_to_string(system_dir.join("build.prop")).is_ok_and(|props| {
        property(&props, "ro.system.build.fingerprint").is_some_and(|v| v.starts_with("Kemu/"))
    })
}

pub(crate) fn amd_uma_adapter(gpu: Gpu, adapter: &str) -> bool {
    matches!(gpu, Gpu::Host | Gpu::Auto) && adapter == "AMD Radeon 780M Graphics"
}

pub(crate) fn amd_uma_sdk_image(sdk: &Sdk, image: &Path) -> bool {
    let expected = sdk
        .root
        .join("system-images/android-37.0/google_apis_playstore/x86_64");
    image.join("system.img").is_file()
        && fs::canonicalize(expected)
            .is_ok_and(|path| fs::canonicalize(image).ok().as_ref() == Some(&path))
        && fs::read_to_string(image.join("source.properties")).is_ok_and(|meta| {
            property(&meta, "SystemImage.TagId") == Some("google_apis_playstore")
                && property(&meta, "SystemImage.Abi") == Some("x86_64")
        })
        && !is_kemu_image(image)
        && fs::read_to_string(sdk.root.join("emulator/lib/advancedFeatures.ini"))
            .is_ok_and(|meta| property(&meta, "VulkanAllocateDeviceMemoryOnly").is_some())
}

fn apply_amd_uma_vulkan(
    command: &mut Command,
    enabled: bool,
    windows_sdk: bool,
    gpu: Gpu,
    adapter: &str,
    sdk: &Sdk,
    image: &Path,
) {
    if enabled && windows_sdk && amd_uma_adapter(gpu, adapter) && amd_uma_sdk_image(sdk, image) {
        command.args(["-feature", "-VulkanAllocateDeviceMemoryOnly"]);
    }
}

fn should_configure_pubg_angle(boot_log: &Path, linux: bool) -> bool {
    !linux
        || !fs::read(boot_log)
            .is_ok_and(|log| selected_vulkan_is_cpu(&String::from_utf8_lossy(&log)))
}

fn selected_vulkan_is_cpu(log: &str) -> bool {
    // Host GLES may use a different adapter. Require the actual Vulkan selection
    // and its matching physical device type from the same emulator boot.
    let selected = log.lines().rev().find_map(|line| {
        line.split_once("Selecting Vulkan device: ")
            .map(|(_, selection)| selection)
    });
    let Some((selected, _)) = selected.and_then(|entry| entry.rsplit_once(", Version: ")) else {
        return false;
    };
    let mut matching_types = log
        .lines()
        .filter_map(|line| {
            let (_, device) = line.split_once("Found physical GPU '")?;
            let (name, properties) = device.rsplit_once("', type: ")?;
            let (device_type, _) = properties.split_once(',')?;
            (selected == name).then_some(device_type)
        })
        .peekable();
    matching_types.peek().is_some()
        && matching_types.all(|kind| kind == "VK_PHYSICAL_DEVICE_TYPE_CPU")
}

fn matching_kemu_image(root: &Path, system: &Path) -> bool {
    root.join(KEMU_EMULATOR).is_file()
        && same_directory(&root.join(KEMU_IMAGE), system)
        && is_kemu_image(system)
}

pub fn boot(
    sdk: &Sdk,
    profile: &Profile,
    options: BootOptions,
    stop: &AtomicBool,
    mut notify: impl FnMut(BootEvent),
) -> Result<()> {
    let BootOptions {
        gpu,
        hardware,
        amd_uma_vulkan,
        engine,
        headless,
        check,
        embedded,
        timeout,
    } = options;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    sdk.validate_boot_mode(engine, gpu)?;
    profile.ensure_writable_location()?;
    let system_dir = profile.system_dir(sdk)?;
    // This guest Vulkan HAL uses the shared RX/TX extensions of the matching goldfish pipe.
    let installed = sdk.matching_runtime(
        &system_dir,
        engine,
        gpu,
        cfg!(windows).then(kemu_install_root).flatten(),
    )?;
    let runtime = installed.as_deref();
    let gaming;
    let profile = if runtime.is_some() {
        gaming = Profile::localized(profile.root.join("gaming"), sdk.locale);
        gaming.init_disk(
            SystemImage::from_dir_localized(system_dir.clone(), sdk.locale)?,
            true,
        )?;
        &gaming
    } else {
        profile
    };
    let compat_egl = runtime.is_none() && is_kemu_image(&system_dir);
    // Keep this handle for the entire boot/session. The OS releases its lock on exit.
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(profile.root.join("vm-session.lock"))?;
    match lease.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            return Err(sdk
                .locale
                .pick(
                    "Este perfil já está em uso por outra sessão do RustEmuDroid.",
                    "Another RustEmuDroid session is using this profile.",
                    "Otra sesión de RustEmuDroid está usando este perfil.",
                )
                .into());
        }
        Err(fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    // The WHPX user APIC and guest XSAVE workaround were needed on AMD hosts here.
    let amd_whpx = runtime.is_some()
        && env::var("PROCESSOR_IDENTIFIER").is_ok_and(|id| id.contains("AuthenticAMD"));
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let log_path = profile
        .root
        .join(format!("boot-{stamp}-{}.log", std::process::id()));
    let log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&log_path)?;
    let mut command = if let Some(root) = runtime {
        let mut cmd = Command::new(root.join(KEMU_EMULATOR));
        let mut paths: Vec<_> = [
            "lib64/qt/lib",
            "lib64",
            "lib64/gles_swiftshader",
            "lib64/vulkan",
            "",
        ]
        .iter()
        .map(|p| root.join(p))
        .collect();
        if let Some(path) = env::var_os("PATH") {
            paths.extend(env::split_paths(&path));
        }
        cmd.current_dir(&profile.root)
            .env("PATH", env::join_paths(paths)?)
            .env_remove("KEMU_UI")
            .env_remove("KEMU_INSTANCE_HANDLE")
            .env_remove("KEMU_INSTANCE")
            .env("ANDROID_EMULATOR_LAUNCHER_DIR", root)
            .env("QT_PLUGIN_PATH", root.join("lib64/qt/plugins"))
            .env("QT_QPA_PLATFORM_PLUGIN_PATH", root.join("lib64/qt/plugins"))
            .args([
                "-no-boot-anim",
                "-crash-report-mode",
                "disabled",
                "-feature",
                "GuestAngle,VulkanBatchedDescriptorSetUpdate,-MultiDisplay",
            ]);
        if amd_whpx && env::var_os("KEMU_WHPX_APIC").is_none() {
            cmd.env("KEMU_WHPX_APIC", "user");
        }
        cmd
    } else {
        let mut cmd = Command::new(&sdk.emulator);
        if embedded || headless {
            // The SDK can block before ADB starts to ask about a previous crash.
            // The launcher and QEMU both inherit this session-local setting.
            cmd.env("ANDROID_EMU_ENABLE_CRASH_REPORTING", "0");
        }
        cmd
    };
    let sdk_root = runtime.unwrap_or(&sdk.root);
    hardware.apply(&mut command, gpu);
    command
        .env("ANDROID_HOME", sdk_root)
        .env("ANDROID_SDK_ROOT", sdk_root)
        .env("ANDROID_AVD_HOME", &profile.avd_home)
        .env("ANDROID_EMULATOR_HOME", &profile.user_home)
        .env("ANDROID_USER_HOME", &profile.user_home)
        .args([
            "-avd",
            AVD,
            "-accel",
            "on",
            "-gpu",
            gpu.as_str(),
            "-no-snapshot",
            "-no-metrics",
        ])
        .arg("-sysdir")
        .arg(&system_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    if compat_egl {
        // The stock SDK renderer boots this image through its bundled GLES driver.
        command.args(["-feature", "-GuestAngle,-Vulkan,-MultiDisplay"]);
    }
    apply_amd_uma_vulkan(
        &mut command,
        amd_uma_vulkan,
        cfg!(windows) && runtime.is_none() && !compat_egl,
        gpu,
        &hardware.adapter,
        sdk,
        &system_dir,
    );
    if headless {
        command.args(["-no-window", "-no-audio"]);
    }

    let (port, console, adb_port) = free_port_pair()?;
    let serial = format!("emulator-{port}");
    command.arg("-port").arg(port.to_string());
    let rpc_socket = if embedded {
        let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let rpc = socket.local_addr()?.port();
        let display_mode = if cfg!(target_os = "linux") {
            "-no-window"
        } else {
            "-qt-hide-window"
        };
        command
            .args([display_mode, "-grpc-use-token", "-grpc"])
            .arg(rpc.to_string());
        // Linux reads ANDROID_EMULATOR_DISCOVERY_DIR, Windows uses LOCALAPPDATA/Temp.
        command.env("ANDROID_EMULATOR_DISCOVERY_DIR", &profile.user_home);
        Some((socket, rpc))
    } else {
        None
    };
    if compat_egl {
        command.args(["-qemu", "-append", "androidboot.hardwareegl=emulation"]);
    } else if amd_whpx {
        command.args(["-qemu", "-append", "noxsaves"]);
    }
    let rpc = rpc_socket.map(|(_, rpc)| rpc);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // ADB is shared with other emulators. Start its server outside the private VM job.
        let mut adb = RunningProcess(
            Command::new(&sdk.adb)
                .arg("start-server")
                .creation_flags(0x08000000)
                .stdout(Stdio::null())
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            if let Some(status) = adb.0.try_wait()? {
                if !status.success() {
                    return Err(format!(
                        "{}: {status}",
                        sdk.locale.pick(
                            "Falha ao iniciar o ADB",
                            "Could not start ADB",
                            "No se ha podido iniciar ADB"
                        )
                    )
                    .into());
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err(sdk
                    .locale
                    .pick(
                        "O servidor ADB não respondeu ao iniciar.",
                        "The ADB server did not respond during startup.",
                        "El servidor ADB no ha respondido durante el arranque.",
                    )
                    .into());
            }
            sleep(Duration::from_millis(100));
        }
    }
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    drop((console, adb_port));
    let mut vm = RunningProcess(command.spawn()?);
    #[cfg(windows)]
    let _job = vm_job(&vm.0).map_err(|error| {
        format!(
            "{}: {error}",
            sdk.locale.pick(
                "Falha ao vincular a VM ao launcher",
                "Could not attach the VM to the launcher",
                "No se ha podido vincular la VM al cliente"
            )
        )
    })?;

    notify(BootEvent::Starting(format!(
        "{} {AVD} / {serial}. {}: {}. Log: {}",
        sdk.locale.pick("Iniciando", "Starting", "Iniciando"),
        sdk.locale.pick("Motor", "Engine", "Motor"),
        command.get_program().to_string_lossy(),
        log_path.display()
    )));
    if let Some(rpc) = rpc {
        notify(BootEvent::Display {
            console: port,
            rpc,
            avd: profile.avd_home.join(format!("{AVD}.avd")),
            #[cfg(feature = "ui")]
            adb: sdk.adb.clone(),
        });
    }
    let start = Instant::now();
    let mut guest_graphics_ready = !compat_egl;
    let expected_avd = profile.avd_home.join(format!("{AVD}.avd"));
    while start.elapsed() < timeout {
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Some(status) = vm.0.try_wait()? {
            return Err(format!(
                "{} ({status}): {}",
                sdk.locale.pick(
                    "Emulador encerrou antes do boot",
                    "Emulator exited before boot",
                    "El emulador se cerró antes del arranque"
                ),
                log_path.display()
            )
            .into());
        }
        if !owns_console(sdk, &serial, &expected_avd) {
            sleep(POLL_INTERVAL);
            continue;
        }
        if !guest_graphics_ready {
            // This property is stored on the prototype's writable data disk.
            guest_graphics_ready = adb_output(
                sdk,
                &serial,
                [
                    "shell",
                    "su",
                    "0",
                    "setprop",
                    "persist.graphics.egl",
                    "emulation",
                ],
            )
            .is_some();
        }
        if guest_graphics_ready
            && adb_output(sdk, &serial, ["shell", "getprop", "sys.boot_completed"])
                .is_some_and(|output| output.trim() == "1")
        {
            let version = adb_output(
                sdk,
                &serial,
                ["shell", "getprop", "ro.build.version.release"],
            )
            .unwrap_or_else(|| "?".into());
            if version.trim() == "17"
                && matches!(gpu, Gpu::Auto | Gpu::Host)
                && fs::read_to_string(system_dir.join("source.properties"))
                    .ok()
                    .is_some_and(|meta| {
                        property(&meta, "SystemImage.TagId") == Some("google_apis_playstore")
                            && property(&meta, "SystemImage.Abi") == Some("x86_64")
                    })
                && should_configure_pubg_angle(&log_path, cfg!(target_os = "linux"))
            {
                // This guest's native EGL driver can crash on swapBuffers in PUBG.
                // Use the guest's per-app GLES driver selection before exposing Home.
                match adb_output(
                    sdk,
                    &serial,
                    ["shell", include_str!("../android/pubg-angle.sh")],
                )
                .as_deref()
                .map(str::trim)
                {
                    Some("applied") => notify(BootEvent::Message(
                        sdk.locale
                            .pick(
                                "ANGLE sobre Vulkan ativado para o PUBG MOBILE.",
                                "Enabled ANGLE over Vulkan for PUBG MOBILE.",
                                "ANGLE sobre Vulkan activado para PUBG MOBILE.",
                            )
                            .into(),
                    )),
                    None => notify(BootEvent::Message(
                        sdk.locale
                            .pick(
                                "Não foi possível configurar o driver gráfico do PUBG MOBILE.",
                                "Could not configure the graphics driver for PUBG MOBILE.",
                                "No se pudo configurar el controlador gráfico de PUBG MOBILE.",
                            )
                            .into(),
                    )),
                    _ => {}
                }
            }
            if hardware.arm_lite && is_kemu_image(&system_dir) && version.trim() == "16" {
                let bridge = adb_output(
                    sdk,
                    &serial,
                    ["shell", "getprop", "ro.dalvik.vm.native.bridge"],
                );
                if bridge.as_deref().map(str::trim) == Some("libberberis_arm64.so") {
                    const MODE: &str = "lite-translate-or-interpret";
                    let applied = adb_output(
                        sdk,
                        &serial,
                        ["shell", "su", "0", "setprop", "berberis.mode", MODE],
                    )
                    .and_then(|_| adb_output(sdk, &serial, ["shell", "getprop", "berberis.mode"]))
                    .is_some_and(|value| value.trim() == MODE);
                    notify(BootEvent::Message(
                        if applied {
                            sdk.locale.pick(
                                "Tradução ARM leve ativada para este boot.",
                                "Lightweight ARM translation enabled for this boot.",
                                "Traducción ARM ligera activada en este arranque.",
                            )
                        } else {
                            sdk.locale.pick(
                                "O Android não permitiu ativar a tradução ARM leve.",
                                "Android could not enable lightweight ARM translation.",
                                "Android no ha podido activar la traducción ARM ligera.",
                            )
                        }
                        .into(),
                    ));
                }
            }
            notify(BootEvent::Ready(format!(
                "{} {:.1}s ({serial}, Android {}, sys.boot_completed=1).",
                sdk.locale.pick(
                    "Boot confirmado em",
                    "Boot confirmed in",
                    "Arranque confirmado en"
                ),
                start.elapsed().as_secs_f64(),
                version.trim()
            )));
            if let Some(fingerprint) =
                adb_output(sdk, &serial, ["shell", "getprop", "ro.build.fingerprint"])
            {
                notify(BootEvent::Message(format!(
                    "{}: {}",
                    sdk.locale
                        .pick("Imagem Android", "Android image", "Imagen Android"),
                    fingerprint.trim()
                )));
            }

            if !check {
                loop {
                    if let Some(status) = vm.0.try_wait()? {
                        if status.success() {
                            return Ok(());
                        }
                        return Err(format!(
                            "{} ({status}): {}",
                            sdk.locale
                                .pick("Motor encerrou", "Engine exited", "El motor se cerró"),
                            log_path.display()
                        )
                        .into());
                    }
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    sleep(POLL_INTERVAL);
                }
            }
            if !stop_owned_vm(sdk, &serial, &expected_avd) {
                return Err(sdk
                    .locale
                    .pick(
                        "Falha ao pedir o encerramento normal da VM.",
                        "Could not request a clean VM shutdown.",
                        "Error al solicitar el apagado de la VM.",
                    )
                    .into());
            }
            if !wait_for_exit(&mut vm.0, Duration::from_secs(15))? {
                return Err(sdk
                    .locale
                    .pick(
                        "A VM não encerrou após o pedido de desligamento.",
                        "The VM did not exit after the shutdown request.",
                        "La VM sigue activa después de solicitar el apagado.",
                    )
                    .into());
            }
            return Ok(());
        }
        sleep(POLL_INTERVAL);
    }

    if stop_owned_vm(sdk, &serial, &expected_avd) {
        let _ = wait_for_exit(&mut vm.0, Duration::from_secs(10));
    }
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    Err(format!(
        "{} ({serial}): {}",
        sdk.locale.pick(
            "Tempo limite de boot excedido",
            "Boot timed out",
            "Se ha agotado el tiempo de arranque"
        ),
        log_path.display()
    )
    .into())
}

fn owns_console(sdk: &Sdk, serial: &str, expected: &Path) -> bool {
    adb_output(sdk, serial, ["emu", "avd", "path"])
        .is_some_and(|output| output.lines().next().map(str::trim).map(Path::new) == Some(expected))
}

fn stop_owned_vm(sdk: &Sdk, serial: &str, expected: &Path) -> bool {
    if !owns_console(sdk, serial, expected) {
        return false;
    }
    // emu kill can power off before recent APK and preference writes reach the virtual disk.
    // During early boot the shell may still be unavailable. adb_output bounds that wait.
    let _ = adb_output(sdk, serial, ["shell", "sync"]);
    owns_console(sdk, serial, expected) && adb_output(sdk, serial, ["emu", "kill"]).is_some()
}

fn adb_output(
    sdk: &Sdk,
    serial: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Option<String> {
    let mut child = RunningProcess(
        Command::new(&sdk.adb)
            .args(["-s", serial])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?,
    );
    if !wait_for_exit(&mut child.0, Duration::from_secs(4)).ok()? {
        return None;
    }
    let mut stdout = child.0.stdout.take()?;
    let mut output = String::new();
    io::Read::read_to_string(&mut stdout, &mut output).ok()?;
    child.0.wait().ok()?.success().then_some(output)
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> io::Result<bool> {
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        sleep(Duration::from_millis(100));
    }
}

struct RunningProcess(Child);

#[cfg(windows)]
fn vm_job(child: &Child) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    // An unnamed, non-inheritable handle ties the VM to this launcher even if a
    // native fault prevents Rust destructors from running. Normal shutdown still uses ADB.
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedHandle::from_raw_handle(handle);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of_val(&info) as u32,
        ) == 0
            || AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Engine, Gpu, KEMU_EMULATOR, KEMU_IMAGE, Locale, RuntimeKind, Sdk, SystemImage, env, fs,
        parse_image, property,
    };
    use std::path::{Path, PathBuf};

    struct TestRoot(PathBuf);

    #[test]
    fn sdk_acceleration_check_uses_the_windows_helper_when_present() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("SDK with spaces");
        for folder in ["emulator", "platform-tools"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let suffix = env::consts::EXE_SUFFIX;
        let emulator = root.join("emulator").join(format!("emulator{suffix}"));
        let adb = root.join("platform-tools").join(format!("adb{suffix}"));
        for tool in [&emulator, &adb] {
            fs::write(tool, b"").unwrap();
        }
        let sdk = Sdk::from_root(root, RuntimeKind::Portable, Locale::EnUs).unwrap();
        let fallback = sdk.sdk_acceleration_command();
        assert_eq!(fallback.get_program(), emulator.as_os_str());
        assert_eq!(fallback.get_args().collect::<Vec<_>>(), ["-accel-check"]);

        let helper = emulator.with_file_name("emulator-check.exe");
        fs::write(&helper, b"").unwrap();
        let command = sdk.sdk_acceleration_command();
        let (program, argument) = if cfg!(windows) {
            (&helper, "accel")
        } else {
            (&emulator, "-accel-check")
        };
        assert_eq!(command.get_program(), program.as_os_str());
        assert_eq!(command.get_args().collect::<Vec<_>>(), [argument]);
    }

    #[test]
    fn first_run_points_to_runtime_setup_and_preserves_explicit_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("Android").join("Sdk");
        for locale in [Locale::PtBr, Locale::EnUs, Locale::EsEs] {
            let discover = |configured| {
                Sdk::discover(configured, None, Some(missing.clone()), None, locale)
                    .err()
                    .unwrap()
                    .to_string()
            };
            let automatic = discover(None);
            let command = automatic.lines().last().unwrap();
            assert_eq!(
                command,
                if cfg!(windows) {
                    r".\rustemudroid.exe runtime-setup --image google-play"
                } else {
                    "./rustemudroid runtime-setup --image google-play"
                }
            );
            let explicit = discover(Some(missing.clone()));
            assert!(explicit.contains(missing.to_str().unwrap()));
            assert!(!explicit.contains("runtime-setup"));
        }
    }

    impl TestRoot {
        fn new() -> Self {
            let root = env::temp_dir().join(format!(
                "rustemudroid-runtime-test-{}-{}",
                std::process::id(),
                super::SystemTime::now()
                    .duration_since(super::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn file(root: &Path, name: &str, value: &str) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    fn sdk_layout(root: &Path) {
        let suffix = env::consts::EXE_SUFFIX;
        file(root, &format!("emulator/emulator{suffix}"), "");
        file(root, &format!("platform-tools/adb{suffix}"), "");
    }

    fn kemu_layout(root: &Path) {
        file(root, KEMU_EMULATOR, "");
        file(root, "platform-tools/adb.exe", "");
        file(root, "system-img/system.img", "");
        file(
            root,
            "system-img/build.prop",
            "ro.system.build.fingerprint=Kemu/test\nro.product.cpu.abi=x86_64\nro.build.version.sdk=36\n",
        );
    }

    #[test]
    fn portable_runtime_precedes_implicit_installs_but_respects_overrides() {
        let dir = TestRoot::new();
        let portable = dir.0.join("runtime");
        let global = dir.0.join("global");
        let kemu = dir.0.join("kemu");
        sdk_layout(&portable);
        sdk_layout(&global);
        kemu_layout(&kemu);
        let find = |configured| {
            Sdk::discover(
                configured,
                Some(portable.clone()),
                Some(global.clone()),
                Some(kemu.clone()),
                Locale::EnUs,
            )
        };
        assert!(find(None).unwrap().kind == RuntimeKind::Portable);
        assert!(find(Some(portable.join("emulator/.."))).unwrap().kind == RuntimeKind::Portable);
        assert!(find(Some(global.clone())).unwrap().kind == RuntimeKind::Sdk);
        assert!(find(Some(dir.0.join("missing"))).is_err());
        fs::remove_file(portable.join(format!("platform-tools/adb{}", env::consts::EXE_SUFFIX)))
            .unwrap();
        assert_eq!(find(None).unwrap().root, global);
    }

    #[test]
    fn standalone_kemu_requires_its_matching_image_and_hardware_rendering() {
        let dir = TestRoot::new();
        let kemu = dir.0.join("installed-kemu");
        let fake = dir.0.join("other-kemu");
        kemu_layout(&kemu);
        kemu_layout(&fake);
        let sdk = Sdk::discover(
            None,
            None,
            Some(dir.0.join("missing-sdk")),
            Some(kemu.clone()),
            Locale::EnUs,
        )
        .unwrap();
        let own_image = kemu.join(KEMU_IMAGE);
        assert!(sdk.is_standalone_kemu());
        assert_eq!(sdk.adb, kemu.join("platform-tools/adb.exe"));
        let image = SystemImage::preferred(&sdk).unwrap();
        assert!(super::same_directory(
            Path::new(&image.location),
            &own_image
        ));
        assert_eq!(
            sdk.matching_runtime(&own_image, Engine::Auto, Gpu::Host, None)
                .unwrap(),
            Some(kemu),
        );
        let sdk_only = sdk.matching_runtime(&own_image, Engine::Sdk, Gpu::Host, None);
        assert_eq!(
            sdk_only.unwrap_err().to_string(),
            "This mode requires the Android SDK. Set its path with --sdk.",
        );
        for gpu in [Gpu::Software, Gpu::SwiftShader] {
            assert!(
                sdk.matching_runtime(&own_image, Engine::Auto, gpu, None)
                    .is_err()
            );
        }
        assert!(
            sdk.matching_runtime(&fake.join(KEMU_IMAGE), Engine::Auto, Gpu::Auto, None)
                .is_err()
        );
        assert!(SystemImage::from_sdk(&sdk, "system-images;android-36;default;x86_64").is_err());
    }

    #[test]
    fn sdk_still_handles_explicit_engine_and_software_modes_for_kemu_image() {
        let dir = TestRoot::new();
        let sdk_root = dir.0.join("sdk");
        let kemu = dir.0.join("kemu");
        sdk_layout(&sdk_root);
        kemu_layout(&kemu);
        let sdk = Sdk::from_root(sdk_root, RuntimeKind::Sdk, Locale::EnUs).unwrap();
        let image = kemu.join(KEMU_IMAGE);
        let choose = |engine, gpu| {
            sdk.matching_runtime(&image, engine, gpu, Some(kemu.clone()))
                .unwrap()
        };
        assert_eq!(choose(Engine::Auto, Gpu::Auto), Some(kemu.clone()));
        assert!(choose(Engine::Sdk, Gpu::Auto).is_none());
        assert!(choose(Engine::Auto, Gpu::Software).is_none());
        assert!(!super::path_is_inside(&dir.0.join("outside"), &kemu));
        assert!(super::path_is_inside(&kemu.join("new/state"), &kemu));
    }

    #[test]
    fn prefers_known_google_images_then_highest_numeric_aosp_version() {
        let dir = TestRoot::new();
        sdk_layout(&dir.0);
        let sdk = Sdk::from_root(dir.0.clone(), RuntimeKind::Portable, Locale::EnUs).unwrap();
        for level in ["9", "100", "100.2", "100.10", "999.bad"] {
            file(
                &dir.0,
                &format!("system-images/android-{level}/default/x86_64/system.img"),
                "",
            );
        }
        assert_eq!(SystemImage::preferred(&sdk).unwrap().api, "android-100.10");
        file(
            &dir.0,
            "system-images/android-35/google_apis/x86_64/system.img",
            "",
        );
        let image = SystemImage::preferred(&sdk).unwrap();
        assert_eq!(
            (image.api.as_str(), image.tag.as_str()),
            ("android-35", "google_apis")
        );
    }

    #[test]
    fn pubg_angle_skips_only_a_confirmed_linux_cpu_vulkan_selection() {
        use super::should_configure_pubg_angle;

        let folder = tempfile::tempdir().unwrap();
        let log = folder.path().join("boot.log");
        let cpu = "INFO | Found physical GPU 'llvmpipe (LLVM 21.1.8, 256 bits)', type: VK_PHYSICAL_DEVICE_TYPE_CPU, apiVersion: 1.4.363\n";
        let amd = "INFO | Found physical GPU 'AMD Radeon 780M Graphics', type: VK_PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU, apiVersion: 1.4.359\n";
        let choose_cpu =
            "INFO | Selecting Vulkan device: llvmpipe (LLVM 21.1.8, 256 bits), Version: 1.4.363\n";
        let choose_amd =
            "INFO | Selecting Vulkan device: AMD Radeon 780M Graphics, Version: 1.4.359\n";
        let choose_unknown =
            "INFO | Selecting Vulkan device: An unclassified device, Version: 1.3.0\n";

        assert!(should_configure_pubg_angle(&log, true));
        for (text, expected) in [
            (format!("{cpu}{amd}{choose_cpu}"), false),
            (format!("{cpu}{amd}{choose_amd}"), true),
            (format!("{cpu}{amd}{choose_cpu}{choose_amd}"), true),
            (format!("{cpu}{amd}{choose_amd}{choose_cpu}"), false),
            (format!("{cpu}{choose_unknown}"), true),
            (format!("{cpu}{choose_cpu}{choose_unknown}"), true),
            (cpu.to_owned(), true),
        ] {
            fs::write(&log, &text).unwrap();
            assert_eq!(should_configure_pubg_angle(&log, true), expected, "{text}");
            assert!(should_configure_pubg_angle(&log, false), "{text}");
        }

        // A matching name with conflicting device types is inconclusive.
        let conflict = cpu.replace("DEVICE_TYPE_CPU", "DEVICE_TYPE_DISCRETE_GPU");
        fs::write(&log, format!("{cpu}{conflict}{choose_cpu}")).unwrap();
        assert!(should_configure_pubg_angle(&log, true));
    }

    #[cfg(feature = "ui")]
    #[test]
    fn amd_uma_vulkan_flag_requires_explicit_opt_in_and_the_tested_sdk_profile() {
        use super::{Gpu, Locale, Sdk, apply_amd_uma_vulkan, fs};
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let sdk = Sdk {
            root: dir.path().to_path_buf(),
            emulator: dir.path().join("emulator/emulator"),
            adb: dir.path().join("platform-tools/adb"),
            locale: Locale::default(),
            kind: super::RuntimeKind::Sdk,
        };
        let features = sdk.root.join("emulator/lib/advancedFeatures.ini");
        let image = sdk
            .root
            .join("system-images/android-37.0/google_apis_playstore/x86_64");
        fs::create_dir_all(features.parent().unwrap()).unwrap();
        fs::create_dir_all(&image).unwrap();
        fs::write(&features, "VulkanAllocateDeviceMemoryOnly = off\n").unwrap();
        fs::write(image.join("system.img"), "").unwrap();
        fs::write(
            image.join("source.properties"),
            "SystemImage.TagId=google_apis_playstore\nSystemImage.Abi=x86_64\n",
        )
        .unwrap();

        let adapter = "AMD Radeon 780M Graphics";
        let args = |enabled, windows_sdk, gpu, adapter, image: &std::path::Path| {
            let mut command = Command::new(&sdk.emulator);
            apply_amd_uma_vulkan(
                &mut command,
                enabled,
                windows_sdk,
                gpu,
                adapter,
                &sdk,
                image,
            );
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        for gpu in [Gpu::Host, Gpu::Auto] {
            assert_eq!(
                args(true, true, gpu, adapter, &image),
                ["-feature", "-VulkanAllocateDeviceMemoryOnly"]
            );
        }
        assert!(args(false, true, Gpu::Host, adapter, &image).is_empty());
        assert!(args(true, false, Gpu::Host, adapter, &image).is_empty());
        assert!(args(true, true, Gpu::Software, adapter, &image).is_empty());
        assert!(args(true, true, Gpu::SwiftShader, adapter, &image).is_empty());
        assert!(args(true, true, Gpu::Auto, "", &image).is_empty());
        assert!(args(true, true, Gpu::Host, "AMD Radeon 890M Graphics", &image).is_empty());
        assert!(args(true, true, Gpu::Host, adapter, &sdk.root).is_empty());
        fs::write(
            image.join("build.prop"),
            "ro.system.build.fingerprint=Kemu/test\n",
        )
        .unwrap();
        assert!(args(true, true, Gpu::Host, adapter, &image).is_empty());
        fs::remove_file(image.join("build.prop")).unwrap();
        fs::write(&features, "# VulkanAllocateDeviceMemoryOnly = off\n").unwrap();
        assert!(args(true, true, Gpu::Host, adapter, &image).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn losing_the_vm_job_handle_terminates_its_child() {
        use std::{os::windows::process::CommandExt, process::Command, time::Duration};
        let mut child = super::RunningProcess(
            Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Start-Sleep -Seconds 30",
                ])
                .creation_flags(0x08000000)
                .spawn()
                .unwrap(),
        );
        let job = super::vm_job(&child.0).unwrap();
        assert!(child.0.try_wait().unwrap().is_none());
        drop(job);
        assert!(super::wait_for_exit(&mut child.0, Duration::from_secs(3)).unwrap());
    }

    #[test]
    fn reads_settings_after_android_emulator_reformats_them() {
        assert_eq!(
            property(
                "image.sysdir.1 = C:\\Program Files\\Kemu\\system-img\r\n",
                "image.sysdir.1"
            ),
            Some("C:\\Program Files\\Kemu\\system-img")
        );
    }

    #[test]
    fn accepts_installed_android_image_package_ids() {
        assert_eq!(
            parse_image("system-images;android-35;google_apis;x86_64").unwrap(),
            ("android-35", "google_apis")
        );
        assert!(parse_image("system-images;android-36;default;x86_64").is_ok());
        assert!(parse_image("system-images;android-37.0;google_apis_playstore;x86_64").is_ok());
    }

    #[test]
    fn rejects_path_components_and_other_abis() {
        for image in [
            "system-images;android-35/../../;google_apis;x86_64",
            "system-images;android-35;../../../avd;x86_64",
            "system-images;android-35;google_apis;arm64-v8a",
        ] {
            assert!(parse_image(image).is_err(), "{image}");
        }
    }
}
