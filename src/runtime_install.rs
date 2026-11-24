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
    pub licenses: Vec<String>,
    pub(crate) components: Vec<Component>,
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

pub(crate) use packages::install;

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

// sdklib saves the SHA-1 of each accepted license text as a line in its licenses directory.
fn previously_accepted(licenses: &[String], dir: &Path) -> Result<bool> {
    let mut recorded = std::collections::HashSet::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.metadata()?.len() <= 1024 * 1024 {
            recorded.extend(
                fs::read_to_string(entry.path())?
                    .split_whitespace()
                    .map(str::to_owned),
            );
        }
    }
    Ok(!licenses.is_empty()
        && licenses
            .iter()
            .all(|s| recorded.contains(&format!("{:x}", Sha1::digest(s.as_bytes())))))
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
            println!("\n{license}");
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
    fn existing_acceptance_covers_each_exact_license_text() {
        let folder = tempfile::tempdir().unwrap();
        let terms = vec!["Terms A\n".to_owned(), "Terms B\n".to_owned()];
        fs::write(
            folder.path().join("first"),
            format!("{:x}\r\n", Sha1::digest(&terms[0])),
        )
        .unwrap();
        assert!(!previously_accepted(&terms, folder.path()).unwrap());
        fs::write(
            folder.path().join("second"),
            format!("{:x}\n", Sha1::digest(&terms[1])),
        )
        .unwrap();
        assert!(previously_accepted(&terms, folder.path()).unwrap());
        assert!(!previously_accepted(&["Terms A".into()], folder.path()).unwrap());
        assert!(!previously_accepted(&[], folder.path()).unwrap());
    }
}
