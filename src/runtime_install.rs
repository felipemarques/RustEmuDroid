//! Opt-in installation of the three Google packages used by the native player.
mod catalog;
mod packages;

use crate::{Result, locale::Locale};
use clap::{Args, ValueEnum};
use sha1::{Digest, Sha1};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum ImageChoice {
    Original,
    GooglePlay,
}

impl ImageChoice {
    pub(crate) const fn package(self) -> &'static str {
        match self {
            Self::Original => "system-images;android-35;default;x86_64",
            Self::GooglePlay => "system-images;android-37.0;google_apis_playstore;x86_64",
        }
    }
}

#[derive(Debug)]
pub(crate) struct InstallPlan {
    pub image: String,
    pub total_bytes: u64,
    pub licenses: Vec<LicenseTerms>,
    pub(crate) components: Vec<Component>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LicenseTerms {
    pub id: String,
    pub text: String,
}

#[derive(Debug)]
pub(crate) struct Component {
    package: String,
    url: String,
    bytes: u64,
    checksum: String,
    relative: PathBuf,
}

pub(crate) struct Progress {
    pub bytes: u64,
    pub total_bytes: u64,
    pub component: String,
}

pub(crate) fn default_destination() -> Result<PathBuf> {
    let home = if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
    };
    Ok(home
        .filter(|p| p.is_absolute())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?
        .join("RustEmuDroid")
        .join("runtime"))
}

fn client() -> Result<reqwest::blocking::Client> {
    // Blocking response reads each have a deadline, even when a large image takes hours overall.
    Ok(reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(20))
        .build()?)
}

fn fetch_metadata(url: &str) -> Result<String> {
    const MAX_XML: u64 = 24 * 1024 * 1024;
    let response = client()?.get(url).send()?.error_for_status()?;
    let mut xml = String::new();
    response.take(MAX_XML + 1).read_to_string(&mut xml)?;
    if xml.len() as u64 > MAX_XML {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData).into());
    }
    Ok(xml)
}

pub(crate) fn plan(choice: ImageChoice, locale: Locale) -> Result<InstallPlan> {
    catalog::resolve(choice, locale)
}

fn check_cancel(cancel: &AtomicBool, locale: Locale) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(locale
            .pick(
                "Instalação cancelada.",
                "Installation canceled.",
                "Instalación cancelada.",
            )
            .into());
    }
    Ok(())
}

#[cfg(feature = "ui")]
pub(crate) use packages::can_add_image;
pub(crate) use packages::{install, owns_runtime_tools};

#[derive(Args)]
pub(crate) struct SetupArgs {
    #[arg(long, value_enum, default_value_t = ImageChoice::Original)]
    image: ImageChoice,
    #[arg(long)]
    destination: Option<PathBuf>,
    #[arg(long, conflicts_with = "licenses_from")]
    accept_license: bool,
    #[arg(long, value_name = "DIR")]
    licenses_from: Option<PathBuf>,
}

// Android repository XML uses TrimStringAdapter for license values before SDK License.getLicenseHash.
fn sdk_license_hash(text: &str) -> String {
    let mut unindented = String::with_capacity(text.len());
    let mut previous_whitespace = false;
    for ch in text.chars() {
        if !(previous_whitespace && matches!(ch, ' ' | '\t')) {
            unindented.push(ch);
        }
        previous_whitespace = matches!(ch, ' ' | '\t'..='\r');
    }
    let mut normalized = String::with_capacity(unindented.len());
    let mut chars = unindented.chars().peekable();
    let mut previous_newline = false;
    while let Some(ch) = chars.next() {
        let single_newline = ch == '\n' && !previous_newline && chars.peek() != Some(&'\n');
        let value = if single_newline { ' ' } else { ch };
        if value != ' ' || !normalized.ends_with(' ') {
            normalized.push(value);
        }
        previous_newline = ch == '\n';
    }
    let value = normalized.trim_matches(|ch| ch <= '\u{20}');
    format!("{:x}", Sha1::digest(value.as_bytes()))
}

fn previously_accepted(licenses: &[LicenseTerms], dir: &Path) -> Result<bool> {
    for license in licenses {
        // Catalog resolution restricts each license identifier to one SDK filename.
        let path = dir.join(&license.id);
        let info = match fs::symlink_metadata(&path) {
            Ok(info) => info,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if !info.is_file() || info.len() > 1024 * 1024 {
            return Ok(false);
        }
        let expected = sdk_license_hash(&license.text);
        if !fs::read_to_string(path)?
            .lines()
            .any(|line| line == expected)
        {
            return Ok(false);
        }
    }
    Ok(!licenses.is_empty())
}

pub(crate) fn command(args: &SetupArgs, locale: Locale) -> Result<()> {
    let plan = plan(args.image, locale)?;
    let destination = match &args.destination {
        Some(dir) => std::path::absolute(dir)?,
        None => default_destination()?,
    };
    println!(
        "{}\n{}\n{} MiB",
        plan.image,
        destination.display(),
        plan.total_bytes.div_ceil(1024 * 1024)
    );
    if let Some(licenses) = &args.licenses_from {
        if !previously_accepted(&plan.licenses, licenses)? {
            return Err(locale
                .pick(
                    "Os termos atuais não constam nos aceites da pasta indicada.",
                    "The supplied directory has no acceptance for the current terms.",
                    "La carpeta indicada no contiene la aceptación de las condiciones actuales.",
                )
                .into());
        }
    } else if !args.accept_license {
        for license in &plan.licenses {
            println!("\n{}", license.text);
        }
        println!(
            "\n{}",
            locale.pick(
                "Para aceitar estes termos e instalar, repita com --accept-license.",
                "To accept these terms and install, repeat with --accept-license.",
                "Para aceptar estas condiciones e instalar, repite el comando con --accept-license."
            )
        );
        return Ok(());
    }
    let mut previous = None;
    install(
        &plan,
        &destination,
        |p| {
            let percent = p.bytes.saturating_mul(100) / p.total_bytes.max(1);
            if previous.as_ref() != Some(&(p.component.clone(), percent)) {
                println!("{}  {percent}%", p.component);
                previous = Some((p.component, percent));
            }
        },
        &AtomicBool::new(false),
        locale,
    )?;
    println!(
        "{} {}",
        locale.pick(
            "Runtime instalado em",
            "Runtime installed in",
            "Entorno instalado en"
        ),
        destination.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_acceptance_uses_the_sdk_license_id_and_layout() {
        let folder = tempfile::tempdir().unwrap();
        let terms = [
            LicenseTerms {
                id: "one".into(),
                text: "  Terms\n A  \n\nB\n".into(),
            },
            LicenseTerms {
                id: "two".into(),
                text: "Terms B".into(),
            },
        ];
        let first = format!("{:x}", Sha1::digest(b"Terms A \n\nB"));
        let second = format!("{:x}", Sha1::digest(b"Terms B"));
        fs::write(
            folder.path().join("one"),
            format!("{first}\r\n{second}\r\n"),
        )
        .unwrap();
        assert!(!previously_accepted(&terms, folder.path()).unwrap());
        fs::write(folder.path().join("two"), format!("{second}\n")).unwrap();
        assert!(previously_accepted(&terms, folder.path()).unwrap());
        let changed = [LicenseTerms {
            id: "one".into(),
            text: "Terms A \n\nC".into(),
        }];
        assert!(!previously_accepted(&changed, folder.path()).unwrap());
        assert!(!previously_accepted(&[], folder.path()).unwrap());
    }

    #[test]
    #[ignore = "reads the live Google catalog and SDK licenses from RUSTEMUDROID_TEST_SDK_LICENSES"]
    fn checks_live_terms_against_an_existing_sdk() -> Result<()> {
        let directory = PathBuf::from(env::var("RUSTEMUDROID_TEST_SDK_LICENSES")?);
        let plan = plan(ImageChoice::Original, Locale::EnUs)?;
        assert!(previously_accepted(&plan.licenses, &directory)?);
        for license in &plan.licenses {
            println!("{} {}", license.id, sdk_license_hash(&license.text));
        }
        Ok(())
    }
}
