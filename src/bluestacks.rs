#[cfg(feature = "ui")]
pub(crate) mod mirror_control;
#[cfg(feature = "ui")]
mod mirror_foreground;
#[cfg(feature = "ui")]
pub(crate) mod mirror_session;
#[cfg(feature = "ui")]
pub(crate) mod mirror_target;
#[cfg(feature = "ui")]
pub(crate) mod player_process;
#[cfg(feature = "ui")]
pub(crate) mod preview;

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt, fs, io,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    process::{Command, Stdio},
    ptr,
};

use windows_sys::Win32::{
    Foundation::ERROR_SUCCESS,
    System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW},
};

use crate::locale::Locale;

#[derive(Debug)]
pub(crate) enum BlueStacksError {
    InstallationMissing,
    NoInstances,
    UnknownInstance(String),
    InvalidPackage,
    ReadConfig(io::Error),
    Start(io::Error),
}

impl BlueStacksError {
    pub(crate) fn localized(&self, locale: Locale) -> String {
        match self {
            Self::InstallationMissing => locale
                .pick(
                    "BlueStacks 5 não foi encontrado.",
                    "BlueStacks 5 was not found.",
                    "No se encontró BlueStacks 5.",
                )
                .into(),
            Self::NoInstances => locale
                .pick(
                    "O BlueStacks ainda não tem instâncias disponíveis.",
                    "No BlueStacks instances are available.",
                    "No hay instancias de BlueStacks disponibles.",
                )
                .into(),
            Self::UnknownInstance(instance) => format!(
                "{} ({instance})",
                locale.pick(
                    "Instância do BlueStacks desconhecida.",
                    "Unknown BlueStacks instance.",
                    "Instancia de BlueStacks desconocida.",
                )
            ),
            Self::InvalidPackage => locale
                .pick(
                    "Informe um pacote Android válido, como org.supertuxkart.stk.",
                    "Enter a valid Android package, such as org.supertuxkart.stk.",
                    "Introduce un paquete de Android válido, como org.supertuxkart.stk.",
                )
                .into(),
            Self::ReadConfig(error) => format!(
                "{} ({error})",
                locale.pick(
                    "Falha ao ler as instâncias do BlueStacks.",
                    "Could not read BlueStacks instances.",
                    "No se pudieron leer las instancias de BlueStacks.",
                )
            ),
            Self::Start(error) => format!(
                "{} ({error})",
                locale.pick(
                    "Falha ao iniciar o BlueStacks.",
                    "Could not start BlueStacks.",
                    "No se pudo iniciar BlueStacks.",
                )
            ),
        }
    }
}

impl fmt::Display for BlueStacksError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.localized(Locale::default()))
    }
}

impl std::error::Error for BlueStacksError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Instance {
    pub(crate) id: String,
    pub(crate) name: String,
}

struct Installation {
    player: PathBuf,
    config: PathBuf,
}

impl Installation {
    fn discover() -> Result<Self, BlueStacksError> {
        let player = registry_path("InstallDir")
            .into_iter()
            .chain(
                env::var_os("ProgramW6432")
                    .or_else(|| env::var_os("ProgramFiles"))
                    .map(|root| PathBuf::from(root).join("BlueStacks_nxt")),
            )
            .map(|dir| dir.join("HD-Player.exe"))
            .find(|path| path.is_file())
            .ok_or(BlueStacksError::InstallationMissing)?;
        let mut configs = registry_path("UserDefinedDir")
            .into_iter()
            .chain(registry_path("DataDir").and_then(|data| data.parent().map(|p| p.to_owned())))
            .chain(
                env::var_os("ProgramData").map(|root| PathBuf::from(root).join("BlueStacks_nxt")),
            )
            .map(|dir| dir.join("bluestacks.conf"))
            .peekable();
        let first = configs
            .peek()
            .cloned()
            .ok_or(BlueStacksError::InstallationMissing)?;
        let config = configs.find(|path| path.is_file()).unwrap_or(first);
        Ok(Self { player, config })
    }

    fn instances(&self) -> Result<Vec<Instance>, BlueStacksError> {
        let text = fs::read_to_string(&self.config).map_err(BlueStacksError::ReadConfig)?;
        let instances = configured_instances(&text);
        if instances.is_empty() {
            return Err(BlueStacksError::NoInstances);
        }
        Ok(instances)
    }
}

fn registry_path(name: &str) -> Option<PathBuf> {
    let subkey: Vec<u16> = r"SOFTWARE\BlueStacks_nxt"
        .encode_utf16()
        .chain([0])
        .collect();
    let value: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY;
    let mut size = 0;
    // Query the byte count, then read into an initialized UTF-16 buffer.
    unsafe {
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let mut data = vec![0_u16; (size as usize).div_ceil(2)];
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut size,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let end = data.iter().position(|code| *code == 0)?;
        (end != 0).then(|| PathBuf::from(OsString::from_wide(&data[..end])))
    }
}

fn configured_instances(config: &str) -> Vec<Instance> {
    let mut names = BTreeMap::new();
    let mut preferred = None;
    for line in config.trim_start_matches('\u{feff}').lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if key == "bst.installed_images" {
            preferred = value
                .split(',')
                .map(str::trim)
                .next()
                .filter(|id| !id.is_empty());
        }
        let Some(id) = key
            .strip_prefix("bst.instance.")
            .and_then(|key| key.strip_suffix(".display_name"))
        else {
            continue;
        };
        if !id.is_empty()
            && id
                .bytes()
                .all(|part| part.is_ascii_alphanumeric() || part == b'_')
        {
            names.insert(id, if value.is_empty() { id } else { value });
        }
    }
    let mut instances: Vec<_> = names
        .into_iter()
        .map(|(id, name)| Instance {
            id: id.into(),
            name: name.into(),
        })
        .collect();
    if let Some(index) = instances
        .iter()
        .position(|instance| Some(instance.id.as_str()) == preferred)
    {
        instances[..=index].rotate_right(1);
    }
    instances
}

pub(crate) fn valid_package(package: &str) -> bool {
    package.contains('.')
        && package.len() <= 255
        && package.split('.').all(|part| {
            let mut chars = part.bytes();
            chars
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && chars.all(|character| character.is_ascii_alphanumeric() || character == b'_')
        })
}

#[cfg(feature = "ui")]
pub(crate) fn mapping_file(state_root: &std::path::Path, instance: &str) -> Option<PathBuf> {
    mirror_target::valid_instance(instance)
        .then(|| state_root.join(format!("bluestacks-{instance}-touch-maps.json")))
}

pub(crate) fn instances() -> Result<Vec<Instance>, BlueStacksError> {
    Installation::discover()?.instances()
}

pub(crate) fn launch(instance: Option<&str>, package: Option<&str>) -> Result<(), BlueStacksError> {
    if package.is_some_and(|package| !valid_package(package)) {
        return Err(BlueStacksError::InvalidPackage);
    }
    let installation = Installation::discover()?;
    let configured = installation.instances()?;
    let chosen = match instance {
        Some(id) => configured
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| BlueStacksError::UnknownInstance(id.into()))?,
        None => &configured[0],
    };

    let mut command = Command::new(&installation.player);
    command
        .args(["--instance", &chosen.id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(package) = package {
        // These are the arguments used by a BlueStacks-generated desktop game shortcut.
        command.args(["--cmd", "launchApp", "--package", package]);
    }
    command.spawn().map_err(BlueStacksError::Start)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_named_instances_and_prefers_the_installed_base_image() {
        let instances = configured_instances(concat!(
            "\u{feff}bst.installed_images=\"Pie64,Nougat32\"\r\n",
            "bst.instance.Nougat32.display_name=\"Android 7\"\r\n",
            "bst.instance.Pie64.display_name=\"Jogos de ação\"\r\n",
            "bst.instance.Pie64_1.display_name=\"\"\r\n",
            "bst.instance...\\rogue.display_name=\"Ignore\"\r\n",
            "bst.instance.Pie64.gpu=\"Ignored setting\"\r\n",
        ));
        assert_eq!(
            instances
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["Pie64", "Nougat32", "Pie64_1"]
        );
        assert_eq!(instances[0].name, "Jogos de ação");
        assert_eq!(instances[2].name, "Pie64_1");
    }

    #[test]
    fn validates_package_without_allowing_command_arguments() {
        assert!(valid_package("org.supertuxkart.stk"));
        assert!(valid_package("com.Example.app_2"));
        for package in [
            "",
            "com",
            "--instance",
            "com.example --cmd",
            "com..example",
            "com.0app",
        ] {
            assert!(!valid_package(package));
        }
    }
}
