use std::{
    fs,
    io::{self, Read, Write},
    num::NonZeroU16,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::{BootEvent, Sdk, adb_output, owns_console};
use crate::locale::Locale;

const PREFIX: &str = "rustemudroid-refresh-floor-v1-";
const KEY: &str = "min_refresh_rate";
const TOGGLE: &str = "debug.graphics.game_default_frame_rate.disabled";

#[derive(Default)]
pub(super) struct Session {
    toggle_before: Option<String>,
    floor: Option<(String, Journal)>,
}

type Shell<'a> = dyn FnMut(&[&str]) -> Option<String> + 'a;

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Unsupported,
    ManualToggle,
    Pending,
}

impl Failure {
    fn message(&self, locale: Locale) -> &'static str {
        match self {
            Self::Unsupported => locale.pick(
                "A taxa rápida exige Android 15 ou posterior e a taxa solicitada confirmada no boot.",
                "High refresh requires Android 15 or newer and confirmation of the requested boot rate.",
                "La frecuencia alta requiere Android 15 o posterior y confirmar la frecuencia solicitada al arrancar.",
            ),
            Self::ManualToggle => locale.pick(
                "O Android recusou a configuração da taxa de jogos. Para usar a taxa alta nesta sessão, ative «Forçar taxa de atualização máxima» e «Desativar o frame rate padrão para jogos» nas Opções do desenvolvedor.",
                "Android did not allow the game frame-rate setting. To use high refresh for this session, enable “Force peak refresh rate” and “Disable default frame rate for games” in Developer options.",
                "Android ha rechazado el ajuste de FPS para juegos. Para usar la frecuencia alta en esta sesión, activa «Forzar tasa de refresco máxima» e «Inhabilitar FPS predeterminada en juegos» en las opciones para desarrolladores.",
            ),
            Self::Pending => locale.pick(
                "Não foi possível configurar ou restaurar a taxa temporária. Uma restauração pendente será tentada no próximo boot deste perfil.",
                "Could not apply or restore the temporary refresh rate. Any pending restoration will be retried on the next boot of this profile.",
                "No se ha podido aplicar o restaurar la frecuencia temporal. La restauración pendiente se reintentará al volver a arrancar el perfil.",
            ),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Journal {
    identity: String,
    imposed: String,
    previous: Option<String>,
}

impl Journal {
    fn read(path: &Path) -> io::Result<Self> {
        let mut contents = String::new();
        fs::File::open(path)?
            .take(1025)
            .read_to_string(&mut contents)?;
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid refresh journal");
        if contents.len() > 1024 {
            return Err(invalid());
        }
        let mut lines = contents.lines();
        let identity = lines
            .next()
            .filter(|line| line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(invalid)?;
        let imposed = lines
            .next()
            .filter(|line| {
                line.strip_suffix(".0")
                    .and_then(|rate| rate.parse::<u16>().ok())
                    .is_some_and(|rate| rate > 60)
            })
            .ok_or_else(invalid)?;
        let previous = preference(lines.next().ok_or_else(invalid)?).ok_or_else(invalid)?;
        if lines.next().is_some() {
            return Err(invalid());
        }
        Ok(Self {
            identity: identity.into(),
            imposed: imposed.into(),
            previous,
        })
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        writeln!(
            file,
            "{}\n{}\n{}",
            self.identity,
            self.imposed,
            self.previous.as_deref().unwrap_or("null")
        )?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path).map_err(|e| e.error)?;
        Ok(())
    }
}

fn preference(raw: &str) -> Option<Option<String>> {
    if raw == "null" {
        return Some(None);
    }
    if raw.len() > 64
        || !raw
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'.' | b'+' | b'-' | b'e' | b'E'))
    {
        return None;
    }
    let number = raw.parse::<f32>().ok()?;
    (number.is_finite() && number >= 0.).then(|| Some(raw.to_owned()))
}

fn read(shell: &mut Shell<'_>, args: &[&str]) -> Option<String> {
    shell(args).map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
}

fn identity(shell: &mut Shell<'_>, user: &str) -> Option<Option<String>> {
    let value = read(
        shell,
        &["settings", "--user", user, "get", "secure", "android_id"],
    )?;
    if value == "null" {
        return Some(None);
    }
    if value.is_empty() || value.len() > 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let id = u64::from_str_radix(&value, 16).ok()?;
    Some(Some(format!("{:x}", Sha256::digest(format!("{id:016x}")))))
}

fn current_user(shell: &mut Shell<'_>) -> Option<String> {
    read(shell, &["am", "get-current-user"]).filter(|s| valid_user(s))
}

fn previous_setting(shell: &mut Shell<'_>, user: &str) -> Option<Option<String>> {
    preference(&read(
        shell,
        &["settings", "--user", user, "get", "system", KEY],
    )?)
}

fn write_setting(shell: &mut Shell<'_>, user: &str, setting: Option<&str>) -> bool {
    let _ = if let Some(value) = setting {
        shell(&["settings", "--user", user, "put", "system", KEY, value])
    } else {
        shell(&["settings", "--user", user, "delete", "system", KEY])
    };
    previous_setting(shell, user).is_some_and(|after| after.as_deref() == setting)
}

fn journals(avd: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let mut pending = Vec::new();
    for entry in fs::read_dir(avd)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some((user, _)) = name
            .to_str()
            .and_then(|name| name.strip_prefix(PREFIX))
            .and_then(|name| name.split_once('-'))
            && valid_user(user)
        {
            if !entry.file_type()?.is_file() {
                return Err(io::ErrorKind::InvalidData.into());
            }
            pending.push((user.into(), entry.path()));
        }
    }
    Ok(pending)
}

fn valid_user(user: &str) -> bool {
    user.parse::<u32>().is_ok_and(|id| user == id.to_string())
}

fn restore_floor(shell: &mut Shell<'_>, user: &str, journal: &Journal) -> Option<()> {
    if identity(shell, user)?.as_deref() != Some(journal.identity.as_str()) {
        return Some(());
    }
    let current = previous_setting(shell, user)?;
    if current.as_deref() != Some(journal.imposed.as_str())
        || write_setting(shell, user, journal.previous.as_deref())
    {
        Some(())
    } else {
        None
    }
}

fn recover(shell: &mut Shell<'_>, avd: &Path) -> Option<()> {
    let mut rollback = Vec::new();
    let mut active_user = None;
    let mut failed = false;
    let mut affects_active_user = |shell: &mut Shell<'_>, user: &str| {
        active_user
            .get_or_insert_with(|| current_user(shell))
            .as_deref()
            .is_none_or(|active| active == user)
    };
    for (user, path) in journals(avd).ok()? {
        let Ok(journal) = Journal::read(&path) else {
            failed = true;
            continue;
        };
        let Some(guest) = identity(shell, &user) else {
            failed |= affects_active_user(shell, &user);
            continue;
        };
        if guest.as_deref() != Some(journal.identity.as_str()) {
            // The profile may have fresh userdata or this Android user may be gone.
            continue;
        }
        let Some(current) = previous_setting(shell, &user) else {
            failed |= affects_active_user(shell, &user);
            continue;
        };
        if current.as_deref() == Some(journal.imposed.as_str()) {
            rollback.push((user, journal));
        } else {
            // A fresh SettingsProvider has now loaded the original or independently edited value.
            failed |= fs::remove_file(path).is_err();
        }
    }
    // Collect matching records before making any changes. Previous runs may have used another
    // rate, and our own restoration must not be mistaken for a write made by that run.
    for (user, journal) in rollback {
        if restore_floor(shell, &user, &journal).is_none() {
            failed |= affects_active_user(shell, &user);
        }
        // SettingsProvider can still have a pending disk write. Keep the intent across shutdown
        // and release it only after a later boot observes a different value.
    }
    (!failed).then_some(())
}

fn toggle(shell: &mut Shell<'_>) -> Option<String> {
    read(shell, &["getprop", TOGGLE]).filter(|value| {
        matches!(
            value.as_str(),
            "" | "0" | "1" | "false" | "true" | "n" | "y" | "no" | "yes" | "off" | "on"
        )
    })
}

fn set_toggle(shell: &mut Shell<'_>, value: &str) -> bool {
    let argument = if value.is_empty() { "''" } else { value };
    let _ = shell(&["setprop", TOGGLE, argument]);
    toggle(shell).as_deref() == Some(value)
}

impl Session {
    fn restore(&mut self, shell: &mut Shell<'_>) -> bool {
        let floor_restored = self
            .floor
            .as_ref()
            .is_none_or(|(user, journal)| restore_floor(shell, user, journal).is_some());
        if floor_restored {
            self.floor = None;
        }
        self.restore_toggle(shell) && floor_restored
    }

    fn restore_toggle(&mut self, shell: &mut Shell<'_>) -> bool {
        let Some(previous) = self.toggle_before.as_deref() else {
            return true;
        };
        let Some(current) = toggle(shell) else {
            return false;
        };
        if current == "true" && !set_toggle(shell, previous) {
            return false;
        }
        self.toggle_before = None;
        true
    }

    fn apply(&mut self, shell: &mut Shell<'_>, avd: &Path, rate: u16) -> Result<(), Failure> {
        let sdk = read(shell, &["getprop", "ro.build.version.sdk"]);
        let nominal = read(shell, &["getprop", "ro.boot.qemu.vsync"]);
        if !sdk.is_some_and(|s| s.parse::<u32>().is_ok_and(|api| api >= 35))
            || !nominal.is_some_and(|s| s.parse::<u16>() == Ok(rate))
        {
            return Err(Failure::Unsupported);
        }
        let user = current_user(shell).ok_or(Failure::Pending)?;
        let guest_id = identity(shell, &user).flatten().ok_or(Failure::Pending)?;
        let previous = previous_setting(shell, &user).ok_or(Failure::Pending)?;

        let original_toggle = toggle(shell).ok_or(Failure::ManualToggle)?;
        if !matches!(original_toggle.as_str(), "true" | "1" | "y" | "yes" | "on") {
            self.toggle_before = Some(original_toggle);
            if !set_toggle(shell, "true") {
                let _ = self.restore_toggle(shell);
                return Err(Failure::ManualToggle);
            }
        }
        if previous.as_deref().and_then(|s| s.parse::<f32>().ok()) == Some(f32::from(rate)) {
            return Ok(());
        }
        let installed = (|| {
            let journal = Journal {
                identity: guest_id,
                imposed: format!("{rate}.0"),
                previous,
            };
            let path = avd.join(format!("{PREFIX}{user}-{}-{rate}", journal.identity));
            let saved = match journal.save(&path) {
                Ok(()) => true,
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists
                        && Journal::read(&path).ok()? == journal =>
                {
                    false
                }
                Err(_) => return None,
            };
            if current_user(shell).as_deref() != Some(user.as_str())
                || identity(shell, &user).flatten().as_deref() != Some(journal.identity.as_str())
                || previous_setting(shell, &user).is_none_or(|current| current != journal.previous)
            {
                if saved {
                    let _ = fs::remove_file(&path);
                }
                return None;
            }
            let active = write_setting(shell, &user, Some(&journal.imposed));
            self.floor = Some((user, journal));
            active.then_some(())
        })();
        if installed.is_none() {
            let _ = self.restore(shell);
            return Err(Failure::Pending);
        }
        Ok(())
    }
}

fn guest(sdk: &Sdk, serial: &str, avd: &Path, args: &[&str]) -> Option<String> {
    if !owns_console(sdk, serial, avd) {
        return None;
    }
    adb_output(
        sdk,
        serial,
        std::iter::once("shell").chain(args.iter().copied()),
    )
}

pub(super) fn boot(
    sdk: &Sdk,
    serial: &str,
    avd: &Path,
    requested: Option<NonZeroU16>,
    notify: &mut impl FnMut(BootEvent),
) -> Session {
    let mut session = Session::default();
    let mut shell = |args: &[&str]| guest(sdk, serial, avd, args);
    let outcome = configure(&mut session, &mut shell, avd, requested);
    if let Err(error) = outcome {
        notify(BootEvent::Message(error.message(sdk.locale).into()));
    }
    session
}

fn configure(
    session: &mut Session,
    shell: &mut Shell<'_>,
    avd: &Path,
    requested: Option<NonZeroU16>,
) -> Result<(), Failure> {
    if recover(shell, avd).is_none() {
        Err(Failure::Pending)
    } else if let Some(rate) = requested.map(NonZeroU16::get).filter(|&rate| rate > 60) {
        session.apply(shell, avd, rate)
    } else {
        Ok(())
    }
}

pub(super) fn shutdown(
    mut session: Session,
    sdk: &Sdk,
    serial: &str,
    avd: &Path,
    notify: &mut impl FnMut(BootEvent),
) {
    let mut shell = |args: &[&str]| guest(sdk, serial, avd, args);
    if !session.restore(&mut shell) {
        notify(BootEvent::Message(
            Failure::Pending.message(sdk.locale).into(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Guest {
        sdk: &'static str,
        nominal: &'static str,
        android_id: &'static str,
        minimum: Option<String>,
        property: String,
        inaccessible_user: Option<&'static str>,
        deny_property: bool,
        deny_floor: bool,
        queries: usize,
        writes: Vec<String>,
    }

    impl Default for Guest {
        fn default() -> Self {
            Self {
                sdk: "37",
                nominal: "90",
                android_id: "123",
                minimum: None,
                property: String::new(),
                inaccessible_user: None,
                deny_property: false,
                deny_floor: false,
                queries: 0,
                writes: Vec::new(),
            }
        }
    }

    impl Guest {
        fn command(&mut self, args: &[&str]) -> Option<String> {
            self.queries += 1;
            match args {
                ["getprop", "ro.build.version.sdk"] => Some(self.sdk.into()),
                ["getprop", "ro.boot.qemu.vsync"] => Some(self.nominal.into()),
                ["am", "get-current-user"] => Some("10".into()),
                ["settings", "--user", user, ..] if Some(*user) == self.inaccessible_user => None,
                ["settings", "--user", "10", "get", "secure", "android_id"] => {
                    Some(self.android_id.into())
                }
                ["settings", "--user", "10", "get", "system", KEY] => {
                    Some(self.minimum.as_deref().unwrap_or("null").into())
                }
                ["getprop", TOGGLE] => Some(self.property.clone()),
                ["setprop", TOGGLE, value] => {
                    self.writes.push(args.join(" "));
                    if self.deny_property {
                        return None;
                    }
                    self.property = if *value == "''" { "" } else { value }.into();
                    Some(String::new())
                }
                ["settings", "--user", "10", "put", "system", KEY, value] => {
                    self.writes.push(args.join(" "));
                    if self.deny_floor {
                        return None;
                    }
                    self.minimum = Some((*value).into());
                    Some(String::new())
                }
                ["settings", "--user", "10", "delete", "system", KEY] => {
                    self.writes.push(args.join(" "));
                    if self.deny_floor {
                        return None;
                    }
                    self.minimum = None;
                    Some(String::new())
                }
                _ => panic!("unexpected command: {args:?}"),
            }
        }

        fn boot(&mut self, avd: &Path, rate: u16) -> Result<Session, Failure> {
            let mut session = Session::default();
            configure(
                &mut session,
                &mut |args| self.command(args),
                avd,
                NonZeroU16::new(rate),
            )?;
            Ok(session)
        }
    }

    #[test]
    fn default_and_sixty_do_not_query_or_write_without_a_journal() {
        let avd = tempfile::tempdir().unwrap();
        let mut guest = Guest::default();
        for rate in [0, 60] {
            let mut session = guest.boot(avd.path(), rate).unwrap();
            assert!(session.restore(&mut |args| guest.command(args)));
        }
        assert_eq!(guest.queries, 0);
    }

    #[test]
    fn unsupported_api_or_mismatched_nominal_never_writes() {
        let avd = tempfile::tempdir().unwrap();
        for mut guest in [
            Guest {
                sdk: "34",
                ..Guest::default()
            },
            Guest {
                nominal: "60",
                ..Guest::default()
            },
        ] {
            assert_eq!(guest.boot(avd.path(), 90).err(), Some(Failure::Unsupported));
            assert!(guest.writes.is_empty());
        }
    }

    #[test]
    fn normal_shutdown_restores_exact_values_and_waits_for_fresh_boot_readback() {
        let avd = tempfile::tempdir().unwrap();
        let mut guest = Guest {
            minimum: Some("48.00".into()),
            property: "0".into(),
            ..Guest::default()
        };
        let mut session = guest.boot(avd.path(), 90).unwrap();
        assert_eq!(guest.minimum.as_deref(), Some("90.0"));
        assert_eq!(guest.property, "true");
        assert!(session.restore(&mut |args| guest.command(args)));
        assert_eq!(guest.minimum.as_deref(), Some("48.00"));
        assert_eq!(guest.property, "0");
        assert_eq!(journals(avd.path()).unwrap().len(), 1);
        guest.writes.clear();
        guest.boot(avd.path(), 0).unwrap();
        assert!(guest.writes.is_empty());
        assert!(journals(avd.path()).unwrap().is_empty());
    }

    #[test]
    fn default_recovers_absence_after_crash_or_a_lost_shutdown_write() {
        let avd = tempfile::tempdir().unwrap();
        let mut guest = Guest::default();
        guest.boot(avd.path(), 90).unwrap();
        // The shell property resets at reboot, while SettingsProvider had persisted the floor.
        guest.property.clear();
        guest.boot(avd.path(), 0).unwrap();
        assert!(guest.minimum.is_none());
        assert_eq!(guest.property, "");
        assert_eq!(journals(avd.path()).unwrap().len(), 1);
        guest.boot(avd.path(), 0).unwrap();
        assert!(journals(avd.path()).unwrap().is_empty());
    }

    #[test]
    fn preserves_independent_edits_and_already_enabled_values() {
        let avd = tempfile::tempdir().unwrap();
        let mut guest = Guest::default();
        let mut session = guest.boot(avd.path(), 90).unwrap();
        guest.minimum = Some("75.0".into());
        guest.property = "false".into();
        guest.writes.clear();
        assert!(session.restore(&mut |args| guest.command(args)));
        guest.boot(avd.path(), 0).unwrap();
        assert_eq!(guest.minimum.as_deref(), Some("75.0"));
        assert_eq!(guest.property, "false");
        assert!(guest.writes.is_empty());

        guest.minimum = Some("90".into());
        guest.property = "yes".into();
        let mut session = guest.boot(avd.path(), 90).unwrap();
        assert!(session.restore(&mut |args| guest.command(args)));
        assert!(guest.writes.is_empty());
        assert!(journals(avd.path()).unwrap().is_empty());
    }

    #[test]
    fn ignores_old_identity_and_applies_for_replacement_userdata() {
        let avd = tempfile::tempdir().unwrap();
        let mut old = Guest {
            minimum: Some("48.00".into()),
            ..Guest::default()
        };
        old.boot(avd.path(), 90).unwrap();
        let mut replacement = Guest {
            android_id: "456",
            minimum: Some("90.0".into()),
            ..Guest::default()
        };
        replacement.boot(avd.path(), 0).unwrap();
        assert!(replacement.writes.is_empty());
        replacement.minimum = Some("60.0".into());
        let mut session = replacement.boot(avd.path(), 90).unwrap();
        assert!(session.restore(&mut |args| replacement.command(args)));
        assert_eq!(replacement.minimum.as_deref(), Some("60.0"));
        assert_eq!(replacement.property, "");
    }

    #[test]
    fn keeps_inaccessible_other_users_journal_and_recovers_the_active_user() {
        let avd = tempfile::tempdir().unwrap();
        let mut guest = Guest::default();
        guest.boot(avd.path(), 90).unwrap();
        let active_path = &journals(avd.path()).unwrap()[0].1;
        let inaccessible_path =
            avd.path()
                .join(active_path.file_name().unwrap().to_str().unwrap().replacen(
                    &format!("{PREFIX}10-"),
                    &format!("{PREFIX}11-"),
                    1,
                ));
        Journal::read(active_path)
            .unwrap()
            .save(&inaccessible_path)
            .unwrap();
        guest.inaccessible_user = Some("11");
        guest.property.clear();
        guest.boot(avd.path(), 0).unwrap();
        assert!(guest.minimum.is_none());

        let mut session = guest.boot(avd.path(), 90).unwrap();
        assert_eq!(guest.minimum.as_deref(), Some("90.0"));
        assert!(session.restore(&mut |args| guest.command(args)));
        assert!(inaccessible_path.is_file());
    }

    #[test]
    fn an_inaccessible_active_users_journal_blocks_new_writes() {
        let avd = tempfile::tempdir().unwrap();
        let mut guest = Guest::default();
        guest.boot(avd.path(), 90).unwrap();
        guest.property.clear();
        guest.writes.clear();
        guest.inaccessible_user = Some("10");
        assert_eq!(guest.boot(avd.path(), 90).err(), Some(Failure::Pending));
        assert!(guest.writes.is_empty());
        assert_eq!(journals(avd.path()).unwrap().len(), 1);
    }

    #[test]
    fn a_failed_property_never_writes_the_floor_and_a_failed_floor_reverts_the_property() {
        let avd = tempfile::tempdir().unwrap();
        let mut guest = Guest {
            minimum: Some("48.00".into()),
            deny_property: true,
            ..Guest::default()
        };
        assert_eq!(
            guest.boot(avd.path(), 90).err(),
            Some(Failure::ManualToggle)
        );
        assert_eq!(guest.writes.len(), 1);
        assert!(guest.writes[0].starts_with("setprop"));
        assert_eq!(guest.minimum.as_deref(), Some("48.00"));
        assert!(journals(avd.path()).unwrap().is_empty());

        guest.deny_property = false;
        guest.deny_floor = true;
        assert_eq!(guest.boot(avd.path(), 90).err(), Some(Failure::Pending));
        assert_eq!(guest.minimum.as_deref(), Some("48.00"));
        assert_eq!(guest.property, "");
        assert_eq!(journals(avd.path()).unwrap().len(), 1);
    }
}
