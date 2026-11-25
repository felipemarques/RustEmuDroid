#[cfg(feature = "ui")]
use super::ImageChoice;
use super::{Component, InstallPlan, Locale, Progress, Result, check_cancel, client};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const RECEIPT: &str = ".rustemudroid-package";
const LOCK: &str = ".rustemudroid-install.lock";

pub(crate) fn owns_runtime_tools(root: &Path) -> bool {
    ["emulator", "platform-tools"].into_iter().all(|package| {
        let receipt = root.join(package).join(RECEIPT);
        fs::symlink_metadata(&receipt).is_ok_and(|info| info.is_file() && info.len() <= 256)
            && fs::read_to_string(receipt).is_ok_and(|receipt| {
                let mut lines = receipt.lines();
                lines.next() == Some(package)
                    && lines.next().is_some_and(|digest| {
                        matches!(digest.len(), 40 | 64)
                            && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            })
    })
}

#[cfg(feature = "ui")]
pub(crate) fn can_add_image(root: &Path, choice: ImageChoice) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let lock = root.join(LOCK);
    if !fs::symlink_metadata(&lock).is_ok_and(|metadata| metadata.is_file())
        || !owns_runtime_tools(&root)
    {
        return false;
    }
    let image = choice
        .package()
        .split(';')
        .fold(root.clone(), |path, segment| path.join(segment));
    [root.join("emulator"), root.join("platform-tools"), image]
        .into_iter()
        .all(|path| {
            path.ancestors()
                .take_while(|parent| *parent != root.as_path())
                .all(|parent| match fs::symlink_metadata(parent) {
                    Ok(_) => parent
                        .canonicalize()
                        .is_ok_and(|actual| actual.starts_with(&root)),
                    Err(error) => error.kind() == io::ErrorKind::NotFound,
                })
        })
}

impl Component {
    fn archive_root(&self) -> &str {
        self.package.split(';').next_back().unwrap_or(&self.package)
    }

    fn check_files(&self, dir: &Path, locale: Locale) -> Result<()> {
        let binary = match self.package.as_str() {
            "emulator" => format!("emulator{}", std::env::consts::EXE_SUFFIX),
            "platform-tools" => format!("adb{}", std::env::consts::EXE_SUFFIX),
            _ => "system.img".into(),
        };
        for name in ["source.properties", &binary] {
            if !dir.join(name).is_file() {
                return Err(format!(
                    "{} {}",
                    locale.pick(
                        "Arquivo ausente em",
                        "Missing file at",
                        "Falta el archivo en"
                    ),
                    dir.join(name).display()
                )
                .into());
            }
        }
        Ok(())
    }

    fn installed(&self, target: &Path, locale: Locale) -> Result<bool> {
        if !target.try_exists()? {
            return Ok(false);
        }
        let receipt = fs::read_to_string(target.join(RECEIPT)).unwrap_or_default();
        if receipt.lines().next() != Some(self.package.as_str()) {
            return Err(format!(
                "{} {}",
                locale.pick(
                    "A pasta já possui uma instalação externa:",
                    "The directory already contains an external installation:",
                    "La carpeta ya contiene una instalación externa:"
                ),
                target.display()
            )
            .into());
        }
        self.check_files(target, locale)?;
        Ok(true)
    }
}

struct CancelReader<'a> {
    file: File,
    cancel: &'a AtomicBool,
}

impl Read for CancelReader<'_> {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("canceled"));
        }
        self.file.read(data)
    }
}

impl Seek for CancelReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

fn unpack(
    component: &Component,
    file: File,
    stage: &Path,
    cancel: &AtomicBool,
    locale: Locale,
) -> Result<PathBuf> {
    let mut archive = zip::ZipArchive::new(CancelReader { file, cancel })?;
    let expected = component.archive_root();
    let prefix = format!("{expected}/");
    if archive.is_empty()
        || !archive
            .file_names()
            .all(|name| name.is_ok_and(|name| name.starts_with(&prefix)))
    {
        return Err(format!(
            "{} {}",
            locale.pick(
                "Estrutura de pacote inesperada em",
                "Unexpected package layout in",
                "Estructura de paquete inesperada en"
            ),
            component.package
        )
        .into());
    }
    let result = archive.extract(stage);
    check_cancel(cancel, locale)?;
    result?;
    let root = stage.join(expected);
    component.check_files(&root, locale)?;
    fs::write(
        root.join(RECEIPT),
        format!("{}\n{}\n", component.package, component.checksum),
    )?;
    Ok(root)
}

fn copy_verified(
    mut reader: impl Read,
    mut file: File,
    component: &Component,
    cancel: &AtomicBool,
    mut report: impl FnMut(u64),
    locale: Locale,
) -> Result<File> {
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let use_sha1 = component.checksum.len() == 40;
    let mut bytes = 0u64;
    let mut buffer = vec![0; 256 * 1024];
    let mut last_report = Instant::now();
    loop {
        check_cancel(cancel, locale)?;
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > component.bytes {
            break;
        }
        file.write_all(&buffer[..n])?;
        if use_sha1 {
            sha1.update(&buffer[..n]);
        } else {
            sha256.update(&buffer[..n]);
        }
        if last_report.elapsed() >= Duration::from_millis(125) {
            report(bytes);
            last_report = Instant::now();
        }
    }
    let checksum = if use_sha1 {
        format!("{:x}", sha1.finalize())
    } else {
        format!("{:x}", sha256.finalize())
    };
    if bytes != component.bytes || !checksum.eq_ignore_ascii_case(&component.checksum) {
        return Err(format!(
            "{} {}",
            locale.pick(
                "Tamanho ou checksum incorreto para",
                "Size or checksum mismatch for",
                "Tamaño o suma de verificación incorrectos para"
            ),
            component.package
        )
        .into());
    }
    file.rewind()?;
    report(bytes);
    Ok(file)
}

pub(crate) fn install(
    plan: &InstallPlan,
    destination: &Path,
    report: impl FnMut(Progress),
    cancel: &AtomicBool,
    locale: Locale,
) -> Result<()> {
    let http = client()?;
    install_from(plan, destination, report, cancel, locale, |component| {
        Ok(http.get(&component.url).send()?.error_for_status()?)
    })
}

fn install_from<R: Read>(
    plan: &InstallPlan,
    destination: &Path,
    mut report: impl FnMut(Progress),
    cancel: &AtomicBool,
    locale: Locale,
    mut open: impl FnMut(&Component) -> Result<R>,
) -> Result<()> {
    check_cancel(cancel, locale)?;
    fs::create_dir_all(destination)?;
    let lock_path = destination.join(LOCK);
    if !lock_path.exists() && fs::read_dir(destination)?.next().transpose()?.is_some() {
        return Err(format!(
            "{} {}",
            locale.pick(
                "Escolha uma pasta vazia para o runtime:",
                "Choose an empty runtime directory:",
                "Elige una carpeta vacía para el entorno:"
            ),
            destination.display()
        )
        .into());
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock().map_err(|err| {
        format!(
            "{} ({err})",
            locale.pick(
                "O instalador não conseguiu reservar esta pasta.",
                "The installer could not lock this directory.",
                "El instalador no pudo bloquear esta carpeta."
            )
        )
    })?;
    let scratch = tempfile::Builder::new()
        .prefix(".rustemudroid-setup-")
        .tempdir_in(destination)?;
    let mut ready = Vec::new();
    let mut completed = 0;
    for (i, component) in plan.components.iter().enumerate() {
        check_cancel(cancel, locale)?;
        let target = destination.join(&component.relative);
        let mut progress = |offset| {
            report(Progress {
                bytes: completed + offset,
                total_bytes: plan.total_bytes,
                component: component.package.clone(),
            })
        };
        if component.installed(&target, locale)? {
            progress(component.bytes);
        } else {
            progress(0);
            let file = tempfile::tempfile_in(scratch.path())?;
            let file = copy_verified(open(component)?, file, component, cancel, progress, locale)?;
            report(Progress {
                bytes: completed + component.bytes,
                total_bytes: plan.total_bytes,
                component: format!(
                    "{} {}",
                    locale.pick("Extraindo", "Extracting", "Extrayendo"),
                    component.package
                ),
            });
            let extracted = unpack(
                component,
                file,
                &scratch.path().join(i.to_string()),
                cancel,
                locale,
            )?;
            ready.push((extracted, target));
        }
        completed += component.bytes;
    }
    check_cancel(cancel, locale)?;
    // Extract and verify everything before publishing directories. Each rename stays on the same
    // filesystem. A retry recognizes already published components after an interrupted commit.
    for (from, to) in ready.into_iter().rev() {
        fs::create_dir_all(to.parent().ok_or("missing component parent")?)?;
        fs::rename(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "offline_test.rs"]
mod offline_test;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    fn fixture(package: &str, executable: &str) -> (Component, Vec<u8>) {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let prefix = package.rsplit(';').next().unwrap();
        for name in [
            format!("{prefix}/source.properties"),
            format!("{prefix}/{executable}"),
        ] {
            zip.start_file(name, SimpleFileOptions::default().unix_permissions(0o755))
                .unwrap();
            zip.write_all(b"fixture").unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        (
            Component {
                package: package.into(),
                url: String::new(),
                bytes: bytes.len() as u64,
                checksum: format!("{:x}", Sha1::digest(&bytes)),
                relative: package.split(';').collect(),
            },
            bytes,
        )
    }

    fn test_zip() -> (Component, Vec<u8>) {
        fixture(
            "platform-tools",
            &format!("adb{}", std::env::consts::EXE_SUFFIX),
        )
    }

    #[test]
    fn verified_archive_keeps_layout_modes_and_reuses_the_committed_package() {
        let (component, bytes) = test_zip();
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let locale = Locale::EnUs;
        let file = copy_verified(
            bytes.as_slice(),
            tempfile::tempfile().unwrap(),
            &component,
            &cancel,
            |_| {},
            locale,
        )
        .unwrap();
        let target = unpack(&component, file, root.path(), &cancel, locale).unwrap();
        assert!(component.installed(&target, locale).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                fs::metadata(target.join("adb"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
        let plan = InstallPlan {
            image: String::new(),
            total_bytes: component.bytes,
            licenses: Vec::new(),
            components: vec![component],
        };
        fs::write(root.path().join(LOCK), "").unwrap();
        let mut done = 0;
        install(&plan, root.path(), |p| done = p.bytes, &cancel, locale).unwrap();
        assert_eq!(done, plan.total_bytes);
    }

    #[test]
    fn corrupt_truncated_and_canceled_downloads_cannot_be_published() {
        let (component, mut bytes) = test_zip();
        let cancel = AtomicBool::new(false);
        bytes[3] ^= 1;
        for bad in [&bytes[..], &bytes[..5]] {
            assert!(
                copy_verified(
                    bad,
                    tempfile::tempfile().unwrap(),
                    &component,
                    &cancel,
                    |_| {},
                    Locale::EnUs
                )
                .is_err()
            );
        }
        cancel.store(true, Ordering::Relaxed);
        let result = copy_verified(
            bytes.as_slice(),
            tempfile::tempfile().unwrap(),
            &component,
            &cancel,
            |_| {},
            Locale::EnUs,
        );
        assert!(result.unwrap_err().to_string().contains("canceled"));
        let foreign = tempfile::tempdir().unwrap();
        fs::write(foreign.path().join("source.properties"), "external").unwrap();
        assert!(component.installed(foreign.path(), Locale::EnUs).is_err());
        assert_eq!(
            fs::read_to_string(foreign.path().join("source.properties")).unwrap(),
            "external"
        );
    }

    #[test]
    fn failing_last_component_leaves_no_partial_runtime_and_retry_publishes_all_three() {
        let root = tempfile::tempdir().unwrap();
        let (emu, emu_zip) = fixture(
            "emulator",
            &format!("emulator{}", std::env::consts::EXE_SUFFIX),
        );
        let (adb, adb_zip) = test_zip();
        let (image, image_zip) = fixture("system-images;android-35;default;x86_64", "system.img");
        let plan = InstallPlan {
            total_bytes: emu.bytes + adb.bytes + image.bytes,
            image: image.package.clone(),
            licenses: vec![],
            components: vec![emu, adb, image],
        };
        let cancel = AtomicBool::new(false);
        let mut damaged = image_zip.clone();
        damaged.pop();
        for (last, success) in [(damaged, false), (image_zip, true)] {
            let mut archives = [emu_zip.as_slice(), adb_zip.as_slice(), &last].into_iter();
            let result = install_from(
                &plan,
                root.path(),
                |_| {},
                &cancel,
                Locale::EnUs,
                |_| Ok(Cursor::new(archives.next().unwrap())),
            );
            assert_eq!(result.is_ok(), success);
            for component in &plan.components {
                assert_eq!(
                    component
                        .installed(&root.path().join(&component.relative), Locale::EnUs)
                        .unwrap(),
                    success
                );
            }
        }
        let sdk =
            crate::android::Sdk::find_localized(Some(root.path().into()), Locale::EnUs).unwrap();
        assert!(sdk.adb.is_file() && sdk.engine_path().is_file());
    }
}
