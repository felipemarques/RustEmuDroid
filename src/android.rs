use std::{
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf, absolute},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::Result;

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
        let SystemImage { api, tag, location } = image;
        let directory = self.avd_home.join(format!("{AVD}.avd"));
        let entry = self.avd_home.join(format!("{AVD}.ini"));
        let profile_path = directory
            .to_str()
            .filter(|path| !path.contains(['\r', '\n']))
            .ok_or("O diretório do AVD deve ser um caminho UTF-8 sem quebras de linha.")?;

        fs::create_dir_all(&directory)?;
        fs::create_dir_all(&self.user_home)?;
        let config = format!(
            "AvdId={AVD}\nabi.type=x86_64\nhw.cpu.arch=x86_64\nhw.cpu.ncore=4\n\
             hw.ramSize=3072\nhw.gpu.enabled=yes\nhw.gpu.mode=auto\nhw.keyboard=yes\n\
             hw.lcd.width=1080\nhw.lcd.height=1920\nhw.lcd.density=420\n\
             disk.dataPartition.size=4G\nPlayStore.enabled=false\n\
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

pub fn boot(
    sdk: &Sdk,
    profile: &Profile,
    gpu: &str,
    headless: bool,
    check: bool,
    timeout: Duration,
) -> Result<()> {
    let system_dir = profile.system_dir(sdk)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let log_path = profile
        .root
        .join(format!("boot-{stamp}-{}.log", std::process::id()));
    let log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&log_path)?;
    let mut command = Command::new(&sdk.emulator);
    command
        .env("ANDROID_HOME", &sdk.root)
        .env("ANDROID_SDK_ROOT", &sdk.root)
        .env("ANDROID_AVD_HOME", &profile.avd_home)
        .env("ANDROID_EMULATOR_HOME", &profile.user_home)
        .env("ANDROID_USER_HOME", &profile.user_home)
        .args([
            "-avd",
            AVD,
            "-accel",
            "on",
            "-gpu",
            gpu,
            "-no-snapshot",
            "-no-metrics",
        ])
        .arg("-sysdir")
        .arg(&system_dir)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    let kemu_image = fs::read_to_string(system_dir.join("build.prop")).is_ok_and(|props| {
        property(&props, "ro.system.build.fingerprint")
            .is_some_and(|fingerprint| fingerprint.starts_with("Kemu/"))
    });
    if kemu_image {
        // The stock SDK renderer boots this image through its bundled GLES driver.
        command.args(["-feature", "-GuestAngle,-Vulkan,-MultiDisplay"]);
    }
    if headless {
        command.args(["-no-window", "-no-audio"]);
    }

    let (port, console, adb_port) = free_port_pair()?;
    let serial = format!("emulator-{port}");
    command.arg("-port").arg(port.to_string());
    if kemu_image {
        command.args(["-qemu", "-append", "androidboot.hardwareegl=emulation"]);
    }
    drop((console, adb_port));
    let mut vm = RunningProcess(command.spawn()?);

    println!("Iniciando {AVD} em {serial}. Log: {}", log_path.display());
    let start = Instant::now();
    let mut guest_graphics_ready = !kemu_image;
    while start.elapsed() < timeout {
        if let Some(status) = vm.0.try_wait()? {
            return Err(format!(
                "Emulador encerrou antes do boot: {status}. Veja {}",
                log_path.display()
            )
            .into());
        }
        if avd_name(sdk, &serial).as_deref() != Some(AVD) {
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
            println!(
                "Boot confirmado em {:.1}s ({serial}, Android {}, sys.boot_completed=1).",
                start.elapsed().as_secs_f64(),
                version.trim()
            );
            if let Some(fingerprint) =
                adb_output(sdk, &serial, ["shell", "getprop", "ro.build.fingerprint"])
            {
                println!("Imagem Android: {}", fingerprint.trim());
            }

            if check {
                if adb_output(sdk, &serial, ["emu", "kill"]).is_none() {
                    return Err("Falha ao pedir o encerramento normal da VM.".into());
                }
                if !wait_for_exit(&mut vm.0, Duration::from_secs(15))? {
                    return Err("A VM não encerrou após o pedido de desligamento.".into());
                }
            } else {
                vm.0.wait()?;
            }
            return Ok(());
        }
        sleep(POLL_INTERVAL);
    }

    if avd_name(sdk, &serial).as_deref() == Some(AVD) {
        let _ = adb_output(sdk, &serial, ["emu", "kill"]);
        let _ = wait_for_exit(&mut vm.0, Duration::from_secs(10));
    }
    Err(format!(
        "Tempo limite de boot excedido em {serial}. Veja {}",
        log_path.display()
    )
    .into())
}

fn avd_name(sdk: &Sdk, serial: &str) -> Option<String> {
    adb_output(sdk, serial, ["emu", "avd", "name"])
        .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned))
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
