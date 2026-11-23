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

use crate::{Engine, Gpu, Result};

const AVD: &str = "RustEmuDroid";
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct Sdk {
    pub root: PathBuf,
    pub adb: PathBuf,
    emulator: PathBuf,
}

impl Sdk {
    pub fn find(explicit: Option<PathBuf>) -> Result<Self> {
        let root = explicit
            .or_else(|| env::var_os("ANDROID_HOME").map(PathBuf::from))
            .or_else(|| env::var_os("ANDROID_SDK_ROOT").map(PathBuf::from))
            .or_else(default_sdk_path)
            .ok_or("Informe --sdk ou configure ANDROID_HOME.")?;
        let root = absolute(root)?;
        let suffix = env::consts::EXE_SUFFIX;
        let emulator = root.join("emulator").join(format!("emulator{suffix}"));
        let adb = root.join("platform-tools").join(format!("adb{suffix}"));
        for tool in [&emulator, &adb] {
            if !tool.is_file() {
                return Err(
                    format!("Ferramenta do Android SDK ausente em {}", tool.display()).into(),
                );
            }
        }
        Ok(Self {
            root,
            emulator,
            adb,
        })
    }

    pub fn check_acceleration(&self) -> Result<()> {
        let status = Command::new(&self.emulator).arg("-accel-check").status()?;
        if !status.success() {
            return Err(format!("Verificação do hipervisor falhou: {status}").into());
        }
        Ok(())
    }
}

fn default_sdk_path() -> Option<PathBuf> {
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
    avd_home: PathBuf,
    user_home: PathBuf,
}

impl Profile {
    pub fn new(root: PathBuf) -> Self {
        Self {
            avd_home: root.join("avd"),
            user_home: root.join("home"),
            root,
        }
    }

    pub fn init(&self, image: SystemImage) -> Result<()> {
        self.init_disk(image, false)
    }

    fn init_disk(&self, image: SystemImage, gaming: bool) -> Result<()> {
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
        let config = format!(
            "AvdId={AVD}\nabi.type=x86_64\nhw.cpu.arch=x86_64\nhw.cpu.ncore=4\n\
             {hardware}\nhw.gpu.enabled=yes\nhw.gpu.mode=auto\nhw.keyboard=yes\nPlayStore.enabled=false\n\
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
        println!("AVD {AVD} pronto em {}", directory.display());
        Ok(())
    }

    fn system_dir(&self, sdk: &Sdk) -> Result<PathBuf> {
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
    pub fn from_sdk(sdk: &Sdk, package: &str) -> Result<Self> {
        let (api, tag) = parse_image(package)?;
        let location = format!("system-images/{api}/{tag}/x86_64/");
        if !sdk.root.join(&location).join("system.img").is_file() {
            return Err(format!(
                "Imagem ausente em {}. Instale {package} pelo SDK Manager.",
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

    pub fn from_dir(path: PathBuf) -> Result<Self> {
        let path = absolute(path)?;
        if !path.join("system.img").is_file() {
            return Err(format!("system.img ausente em {}", path.display()).into());
        }
        let props_path = path.join("build.prop");
        let props = fs::read_to_string(&props_path)
            .map_err(|error| format!("{}: {error}", props_path.display()))?;
        if property(&props, "ro.product.cpu.abi") != Some("x86_64") {
            return Err(format!("{} deve declarar ABI x86_64.", props_path.display()).into());
        }
        let api: u16 = property(&props, "ro.build.version.sdk")
            .ok_or("A imagem deve declarar ro.build.version.sdk.")?
            .parse()?;
        let location = path
            .to_str()
            .filter(|path| !path.contains(['\r', '\n']))
            .ok_or("O caminho da imagem deve ser UTF-8 sem quebras de linha.")?;
        Ok(Self {
            api: format!("android-{api}"),
            tag: "default".into(),
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
        let valid_api = api
            .strip_prefix("android-")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit()));
        if valid_api && matches!(*tag, "default" | "google_apis") {
            return Ok((api, tag));
        }
    }
    Err("Use system-images;android-<API>;google_apis;x86_64 ou a variante default.".into())
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

pub struct BootOptions {
    pub gpu: Gpu,
    pub engine: Engine,
    pub headless: bool,
    pub check: bool,
    pub embedded: bool,
    pub timeout: Duration,
}

pub enum BootEvent {
    Message(String),
    Display {
        console: u16,
        rpc: u16,
        avd: PathBuf,
    },
}

impl std::fmt::Display for BootEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Display { console, rpc, avd } => write!(
                f,
                "Tela integrada em emulator-{console} (RPC {rpc}, {}).",
                avd.display()
            ),
        }
    }
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
        engine,
        headless,
        check,
        embedded,
        timeout,
    } = options;
    let system_dir = profile.system_dir(sdk)?;
    let kemu_image = fs::read_to_string(system_dir.join("build.prop")).is_ok_and(|props| {
        property(&props, "ro.system.build.fingerprint").is_some_and(|v| v.starts_with("Kemu/"))
    });
    // This guest Vulkan HAL uses the shared RX/TX extensions of the matching goldfish pipe.
    let installed = kemu_install_root();
    let runtime = installed.as_deref().filter(|root| {
        cfg!(windows)
            && kemu_image
            && engine == Engine::Auto
            && matches!(gpu, Gpu::Host | Gpu::Auto)
            && root
                .join("qemu/windows-x86_64/qemu-system-x86_64.exe")
                .is_file()
            && fs::canonicalize(root.join("system-img"))
                .is_ok_and(|image| fs::canonicalize(&system_dir).ok().as_ref() == Some(&image))
    });
    let gaming;
    let profile = if runtime.is_some() {
        gaming = Profile::new(profile.root.join("gaming"));
        gaming.init_disk(SystemImage::from_dir(system_dir.clone())?, true)?;
        &gaming
    } else {
        profile
    };
    let compat_egl = kemu_image && runtime.is_none();
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
        let mut cmd = Command::new(root.join("qemu/windows-x86_64/qemu-system-x86_64.exe"));
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
        cmd.current_dir(root)
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
        Command::new(&sdk.emulator)
    };
    let sdk_root = runtime.unwrap_or(&sdk.root);
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
    drop((console, adb_port));
    let mut vm = RunningProcess(command.spawn()?);

    notify(BootEvent::Message(format!(
        "Iniciando {AVD} em {serial}. Motor: {}. Log: {}",
        command.get_program().to_string_lossy(),
        log_path.display()
    )));
    if let Some(rpc) = rpc {
        notify(BootEvent::Display {
            console: port,
            rpc,
            avd: profile.avd_home.join(format!("{AVD}.avd")),
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
                "Emulador encerrou antes do boot: {status}. Veja {}",
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
            notify(BootEvent::Message(format!(
                "Boot confirmado em {:.1}s ({serial}, Android {}, sys.boot_completed=1).",
                start.elapsed().as_secs_f64(),
                version.trim()
            )));
            if let Some(fingerprint) =
                adb_output(sdk, &serial, ["shell", "getprop", "ro.build.fingerprint"])
            {
                notify(BootEvent::Message(format!(
                    "Imagem Android: {}",
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
                            "Motor encerrou com {status}. Veja {}",
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
            if !owns_console(sdk, &serial, &expected_avd)
                || adb_output(sdk, &serial, ["emu", "kill"]).is_none()
            {
                return Err("Falha ao pedir o encerramento normal da VM.".into());
            }
            if !wait_for_exit(&mut vm.0, Duration::from_secs(15))? {
                return Err("A VM não encerrou após o pedido de desligamento.".into());
            }
            return Ok(());
        }
        sleep(POLL_INTERVAL);
    }

    if owns_console(sdk, &serial, &expected_avd) {
        let _ = adb_output(sdk, &serial, ["emu", "kill"]);
        let _ = wait_for_exit(&mut vm.0, Duration::from_secs(10));
    }
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    Err(format!(
        "Tempo limite de boot excedido em {serial}. Veja {}",
        log_path.display()
    )
    .into())
}

fn owns_console(sdk: &Sdk, serial: &str, expected: &Path) -> bool {
    adb_output(sdk, serial, ["emu", "avd", "path"])
        .is_some_and(|output| output.lines().next().map(str::trim).map(Path::new) == Some(expected))
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
    use super::{parse_image, property};

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
