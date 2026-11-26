//! Locating a staged engine folder by its marker file (`staging.json` or the
//! performance engine manifest) and reporting read failures distinctly.
//!
//! `Path::is_file()` turns every error into `false`, so a folder whose ACL the
//! current user cannot read (for example one staged from an elevated session)
//! looked exactly like a folder without the marker. These helpers keep the
//! `io::Error` so the message can say what actually happened.

use std::{fs, io, path::Path};

use super::Locale;

/// Outcome of probing one marker file.
#[derive(Debug)]
pub(super) enum Probe {
    Found,
    Missing,
    Unreadable(io::Error),
}

/// Pure classification of `fs::metadata(path).map(|meta| meta.is_file())`.
pub(super) fn classify(result: io::Result<bool>) -> Probe {
    match result {
        Ok(true) => Probe::Found,
        // A directory with the marker's name is not the marker.
        Ok(false) => Probe::Missing,
        Err(error) if is_missing(&error) => Probe::Missing,
        Err(error) => Probe::Unreadable(error),
    }
}

fn is_missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

pub(super) fn probe(path: &Path) -> Probe {
    classify(fs::metadata(path).map(|meta| meta.is_file()))
}

/// Why no stage folder could be resolved.
#[derive(Debug)]
pub(super) enum StageError<'a> {
    Missing,
    Unreadable(&'a Path, io::Error),
}

/// The folder holding `marker`: `path` itself, or its parent when `path` is
/// the stage's `sdk` subfolder. A read error stops the search and is reported
/// with the folder it happened in.
pub(super) fn find_stage<'a>(
    path: &'a Path,
    marker: &str,
    probe: impl Fn(&Path) -> Probe,
) -> std::result::Result<&'a Path, StageError<'a>> {
    match probe(&path.join(marker)) {
        Probe::Found => return Ok(path),
        Probe::Unreadable(error) => return Err(StageError::Unreadable(path, error)),
        Probe::Missing => {}
    }
    let Some(parent) = path
        .parent()
        .filter(|_| path.file_name().is_some_and(|name| name == "sdk"))
    else {
        return Err(StageError::Missing);
    };
    match probe(&parent.join(marker)) {
        Probe::Found => Ok(parent),
        Probe::Unreadable(error) => Err(StageError::Unreadable(parent, error)),
        Probe::Missing => Err(StageError::Missing),
    }
}

/// Localized text for a failed read of `path` (a folder or a file), always
/// naming the path and the operating system error.
pub(super) fn read_error(path: &Path, error: &io::Error, locale: Locale) -> String {
    let (lead, hint) = if error.kind() == io::ErrorKind::PermissionDenied {
        (
            locale.pick(
                "Sem permissão para ler",
                "No permission to read",
                "Sin permiso para leer",
            ),
            locale.pick(
                " A pasta pode ter sido preparada numa sessão de administrador: dê ao seu usuário permissão de leitura ou prepare-a de novo sem elevação.",
                " The folder may have been prepared from an administrator session: give your user read access or prepare it again without elevation.",
                " Puede que la carpeta se haya preparado desde una sesión de administrador: concede permiso de lectura a tu usuario o vuelve a prepararla sin elevación.",
            ),
        )
    } else if is_missing(error) {
        (
            locale.pick("Não encontrado:", "Not found:", "No encontrado:"),
            "",
        )
    } else {
        (
            locale.pick(
                "Não foi possível ler",
                "Could not read",
                "No se ha podido leer",
            ),
            "",
        )
    };
    format!("{lead} {} ({error}).{hint}", path.display())
}

/// Denies Everyone read access to `path` (and, for a folder, its contents)
/// until dropped. The owner keeps the right to change the ACL, so this needs no
/// elevation and the temporary folder can still be removed afterwards.
#[cfg(all(test, windows))]
pub(super) struct DenyRead(std::path::PathBuf);

#[cfg(all(test, windows))]
impl DenyRead {
    pub(super) fn new(path: &Path) -> Self {
        let rights = if path.is_dir() {
            "*S-1-1-0:(OI)(CI)(R)"
        } else {
            "*S-1-1-0:(R)"
        };
        let status = std::process::Command::new("icacls")
            .arg(path)
            .args(["/deny", rights, "/Q"])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls /deny failed");
        Self(path.to_owned())
    }
}

#[cfg(all(test, windows))]
impl Drop for DenyRead {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(&self.0)
            .args(["/remove:d", "*S-1-1-0", "/T", "/Q"])
            .stdout(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const LOCALES: [Locale; 3] = [Locale::PtBr, Locale::EnUs, Locale::EsEs];

    fn denied() -> io::Error {
        io::Error::from(io::ErrorKind::PermissionDenied)
    }

    #[test]
    fn classifies_metadata_results() {
        assert!(matches!(classify(Ok(true)), Probe::Found));
        assert!(matches!(classify(Ok(false)), Probe::Missing));
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::NotADirectory] {
            assert!(matches!(classify(Err(kind.into())), Probe::Missing));
        }
        assert!(matches!(classify(Err(denied())), Probe::Unreadable(_)));
        assert!(matches!(
            classify(Err(io::ErrorKind::Other.into())),
            Probe::Unreadable(_)
        ));
        // ERROR_ACCESS_DENIED / ERROR_FILE_NOT_FOUND as Windows reports them.
        #[cfg(windows)]
        {
            assert!(matches!(
                classify(Err(io::Error::from_raw_os_error(5))),
                Probe::Unreadable(_)
            ));
            for code in [2, 3] {
                assert!(matches!(
                    classify(Err(io::Error::from_raw_os_error(code))),
                    Probe::Missing
                ));
            }
        }
    }

    #[test]
    fn real_files_are_probed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(probe(&dir.path().join("x.json")), Probe::Missing));
        assert!(matches!(probe(dir.path()), Probe::Missing));
        fs::write(dir.path().join("x.json"), "{}").unwrap();
        assert!(matches!(probe(&dir.path().join("x.json")), Probe::Found));
    }

    #[test]
    fn a_denied_folder_is_not_reported_as_missing() {
        let stage = PathBuf::from(r"D:\engines\stage");
        let sdk = stage.join("sdk");
        // The chosen folder itself is unreadable.
        let error =
            find_stage(&stage, "staging.json", |_| Probe::Unreadable(denied())).unwrap_err();
        assert!(matches!(error, StageError::Unreadable(path, _) if path == stage));
        // The sdk subfolder has no marker, its parent is unreadable.
        let error = find_stage(&sdk, "staging.json", |path| {
            if path.starts_with(&sdk) {
                Probe::Missing
            } else {
                Probe::Unreadable(denied())
            }
        })
        .unwrap_err();
        assert!(matches!(error, StageError::Unreadable(path, _) if path == stage));
        // Missing stays missing; another folder name never looks at the parent.
        assert!(matches!(
            find_stage(&sdk, "staging.json", |_| Probe::Missing),
            Err(StageError::Missing)
        ));
        assert!(matches!(
            find_stage(&stage.join("other"), "staging.json", |path| {
                if path.starts_with(stage.join("other")) {
                    Probe::Missing
                } else {
                    Probe::Found
                }
            }),
            Err(StageError::Missing)
        ));
        assert_eq!(
            find_stage(&sdk, "staging.json", |path| if path.starts_with(&sdk) {
                Probe::Missing
            } else {
                Probe::Found
            })
            .unwrap(),
            stage
        );
    }

    #[test]
    fn read_errors_name_the_path_and_the_os_error_in_every_language() {
        let path = Path::new(r"D:\engines\stage");
        let error = io::Error::new(io::ErrorKind::PermissionDenied, "Access is denied.");
        let texts = LOCALES.map(|locale| read_error(path, &error, locale));
        assert!(
            texts[0].starts_with(r"Sem permissão para ler D:\engines\stage (Access is denied.).")
        );
        assert!(
            texts[1].starts_with(r"No permission to read D:\engines\stage (Access is denied.).")
        );
        assert!(
            texts[2].starts_with(r"Sin permiso para leer D:\engines\stage (Access is denied.).")
        );
        assert!(texts[1].contains("administrator"));
        let other = io::Error::other("bad");
        assert_eq!(
            read_error(path, &other, Locale::EnUs),
            r"Could not read D:\engines\stage (bad)."
        );
        let missing = io::Error::new(io::ErrorKind::NotFound, "gone");
        assert_eq!(
            read_error(path, &missing, Locale::EsEs),
            r"No encontrado: D:\engines\stage (gone)."
        );
    }
}
