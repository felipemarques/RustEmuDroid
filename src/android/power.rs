#![cfg_attr(not(windows), allow(dead_code))]
//! Windows "Best performance" power mode overlay while Android runs.
//!
//! Only the overlay is changed, never the power plan. The previous overlay is
//! kept in a marker file before the first change. Each running session holds
//! a locked lease file; the last session to stop restores the marker's value.
//! If the app died, the next session (or the next desktop start) finds the
//! marker without a live lease and restores it.
//!
//! Windows keeps separate AC and DC overlay values and `powercfg
//! /overlaysetactive` acts on the current power source. The overlay is only
//! turned on while on AC power, the marker records that it was the AC value,
//! and it is only restored while on AC power again; on battery the marker is
//! kept untouched for the next desktop or VM start on AC.
//!
//! The marker lives in a per-user folder rather than in one profile's state
//! folder because the overlay is machine-wide and several profiles (or the
//! CLI and the desktop) can run VMs at the same time.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::Result;

pub(crate) const BEST_PERFORMANCE: &str = "ded574b5-45a0-4f42-8737-46345c09c238";
const MARKER: &str = "previous-overlay.json";
const GUARD: &str = "overlay.lock";
const LEASE_PREFIX: &str = "session-";

/// System access, replaceable in tests.
pub(crate) trait PowerApi {
    /// The AC overlay (`ActiveOverlayAcPowerScheme`), `None` when none is set.
    fn active(&self) -> io::Result<Option<String>>;
    /// Activates an overlay, or clears it with `None`.
    fn set(&self, overlay: Option<&str>) -> io::Result<()>;
    fn on_battery(&self) -> bool;
}

/// Empty and all-zero GUIDs mean "no overlay". GUIDs compare lowercase.
fn normalize(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(['{', '}']).to_ascii_lowercase();
    (!value.is_empty() && value.chars().any(|c| c != '0' && c != '-')).then_some(value)
}

pub(crate) fn default_dir() -> Option<PathBuf> {
    crate::runtime_install::default_destination()
        .ok()
        .and_then(|runtime| runtime.parent().map(|dir| dir.join("power-overlay")))
}

/// The overlay recorded before the first change. Schema 1 (revision 2) only
/// ever recorded the AC value; schema 2 says so explicitly.
fn read_marker(dir: &Path) -> Result<Option<Option<String>>> {
    match fs::read(dir.join(MARKER)) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)?;
            let ac = match value["schema"].as_u64() {
                Some(1) => true,
                Some(2) => value["source"].as_str() == Some("ac"),
                _ => false,
            };
            if !ac {
                return Err(format!("{MARKER}: schema").into());
            }
            Ok(Some(value["previous"].as_str().and_then(normalize)))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_marker(dir: &Path, previous: Option<&str>) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(&serde_json::to_vec(
        &json!({"schema": 2, "source": "ac", "previous": previous}),
    )?)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join(MARKER))?;
    Ok(())
}

/// True when another process or session still holds a lease. Stale leases
/// (their owner exited) are removed.
fn other_live_leases(dir: &Path, own: Option<&Path>) -> io::Result<bool> {
    let mut live = false;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let is_lease = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(LEASE_PREFIX) && name.ends_with(".lock"));
        if !is_lease || Some(path.as_path()) == own {
            continue;
        }
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
            live = true;
            continue;
        };
        match file.try_lock() {
            Ok(()) => {
                drop(file);
                let _ = fs::remove_file(&path);
            }
            Err(_) => live = true,
        }
    }
    Ok(live)
}

fn guard(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(GUARD))?;
    file.lock()?;
    Ok(file)
}

/// What to do with a recorded AC overlay once no session holds a lease.
#[derive(Debug, PartialEq, Eq)]
enum Restore {
    /// No marker.
    Nothing,
    /// On battery: `powercfg` would write the DC value. Keep the marker.
    Wait,
    /// Restore this AC value, then drop the marker.
    Set(Option<String>),
    /// The user picked another mode meanwhile: keep it, drop the marker.
    Forget,
}

fn decide(marker: Option<Option<String>>, on_battery: bool, current: Option<&str>) -> Restore {
    match marker {
        None => Restore::Nothing,
        Some(_) if on_battery => Restore::Wait,
        Some(previous) if current == Some(BEST_PERFORMANCE) => Restore::Set(previous),
        Some(_) => Restore::Forget,
    }
}

/// Restores the recorded overlay when no session holds a lease, which
/// covers a previous crash. Returns whether a marker was handled.
fn restore_if_idle(api: &dyn PowerApi, dir: &Path, own: Option<&Path>) -> Result<bool> {
    let marker = read_marker(dir)?;
    if marker.is_none() || other_live_leases(dir, own)? {
        return Ok(false);
    }
    let on_battery = api.on_battery();
    // The AC value is only read when it matters (not on battery).
    let current = if on_battery { None } else { api.active()? };
    match decide(marker, on_battery, current.as_deref()) {
        Restore::Nothing | Restore::Wait => return Ok(false),
        Restore::Set(previous) => api.set(previous.as_deref())?,
        Restore::Forget => {}
    }
    fs::remove_file(dir.join(MARKER))?;
    Ok(true)
}

/// Call at desktop start to undo an overlay left by a crashed session.
#[cfg_attr(not(feature = "ui"), allow(dead_code))]
pub(crate) fn recover(api: &dyn PowerApi, dir: &Path) -> Result<bool> {
    if !dir.join(MARKER).is_file() {
        return Ok(false);
    }
    let _guard = guard(dir)?;
    restore_if_idle(api, dir, None)
}

/// Held for the whole VM session. Dropping it restores the previous overlay
/// when it is the last session.
pub(crate) struct PowerLease<'a> {
    api: &'a dyn PowerApi,
    dir: PathBuf,
    lease: Option<(File, PathBuf)>,
    active: bool,
}

/// Activates Best performance. Returns `None` on battery power (skipped on
/// purpose: the overlay would mostly drain the battery).
pub(crate) fn acquire<'a>(api: &'a dyn PowerApi, dir: &Path) -> Result<Option<PowerLease<'a>>> {
    let _guard = guard(dir)?;
    restore_if_idle(api, dir, None)?;
    if api.on_battery() {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = dir.join(format!("{LEASE_PREFIX}{}-{stamp}.lock", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.lock()?;
    // Not a PowerLease yet: its Drop would wait for the guard held here.
    let activated = (|| -> Result<bool> {
        if read_marker(dir)?.is_none() {
            // Record first, so a crash right after the change can still be undone.
            let previous = api.active()?;
            write_marker(dir, previous.as_deref())?;
            if previous.as_deref() != Some(BEST_PERFORMANCE) {
                api.set(Some(BEST_PERFORMANCE))?;
            }
        }
        // A later session only adds a lease; the user may have changed the mode.
        Ok(api.active()?.as_deref() == Some(BEST_PERFORMANCE))
    })();
    let active = match activated {
        Ok(active) => active,
        Err(error) => {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
    };
    Ok(Some(PowerLease {
        api,
        dir: dir.to_owned(),
        lease: Some((file, path)),
        active,
    }))
}

impl PowerLease<'_> {
    /// Whether Best performance was the AC overlay when this lease was taken.
    pub(crate) fn overlay_active(&self) -> bool {
        self.active
    }

    fn release(&mut self) -> Result<()> {
        let Some((file, path)) = self.lease.take() else {
            return Ok(());
        };
        let _guard = guard(&self.dir)?;
        drop(file);
        let _ = fs::remove_file(&path);
        restore_if_idle(self.api, &self.dir, Some(&path))?;
        Ok(())
    }
}

impl Drop for PowerLease<'_> {
    // No UI channel reaches here (boot() has returned or is unwinding); a
    // failure leaves the marker, so the next desktop or VM start retries.
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            eprintln!("power overlay: {error}");
        }
    }
}

/// The real Windows implementation: registry read, powercfg write and
/// GetSystemPowerStatus for the battery check.
#[cfg(windows)]
pub(crate) struct System;

#[cfg(windows)]
impl PowerApi for System {
    fn active(&self) -> io::Result<Option<String>> {
        use std::{os::windows::ffi::OsStringExt, ptr};
        use windows_sys::Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
            System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
        };
        let subkey: Vec<u16> = r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes"
            .encode_utf16()
            .chain([0])
            .collect();
        let value: Vec<u16> = "ActiveOverlayAcPowerScheme"
            .encode_utf16()
            .chain([0])
            .collect();
        let mut data = vec![0_u16; 64];
        let mut size = (data.len() * 2) as u32;
        // SAFETY: the buffer and its byte size match; strings are NUL-terminated.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let end = data.iter().position(|c| *c == 0).unwrap_or(data.len());
        Ok(normalize(
            &std::ffi::OsString::from_wide(&data[..end]).to_string_lossy(),
        ))
    }

    fn set(&self, overlay: Option<&str>) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("powercfg.exe")
            .args([
                "/overlaysetactive",
                overlay.unwrap_or("overlay_scheme_none"),
            ])
            .creation_flags(0x08000000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "powercfg /overlaysetactive: {status}"
            )))
        }
    }

    fn on_battery(&self) -> bool {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        #[repr(C)]
        #[derive(Default)]
        #[allow(dead_code)]
        struct PowerStatus {
            ac_line: u8,
            battery_flag: u8,
            battery_percent: u8,
            saver: u8,
            life: u32,
            full_life: u32,
        }
        type Query = unsafe extern "system" fn(*mut PowerStatus) -> i32;
        let kernel32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        let mut status = PowerStatus::default();
        // SAFETY: kernel32 is always loaded; the layout matches SYSTEM_POWER_STATUS.
        unsafe {
            let module = GetModuleHandleW(kernel32.as_ptr());
            let Some(query) = (!module.is_null())
                .then(|| GetProcAddress(module, c"GetSystemPowerStatus".as_ptr().cast()))
                .flatten()
            else {
                return false;
            };
            let query: Query = std::mem::transmute(query);
            // ACLineStatus 0 = offline (battery); 1 = AC; 255 = unknown.
            query(&mut status) != 0 && status.ac_line == 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Fake {
        active: RefCell<Option<String>>,
        calls: RefCell<Vec<Option<String>>>,
        battery: Cell<bool>,
    }

    impl PowerApi for Fake {
        fn active(&self) -> io::Result<Option<String>> {
            Ok(self.active.borrow().clone())
        }
        fn set(&self, overlay: Option<&str>) -> io::Result<()> {
            *self.active.borrow_mut() = overlay.map(str::to_owned);
            self.calls.borrow_mut().push(overlay.map(str::to_owned));
            Ok(())
        }
        fn on_battery(&self) -> bool {
            self.battery.get()
        }
    }

    const BETTER_BATTERY: &str = "961cc777-2547-4f9d-8174-7d86181b8a7a";

    #[test]
    fn overlay_guids_are_normalized() {
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("00000000-0000-0000-0000-000000000000"), None);
        assert_eq!(
            normalize("{DED574B5-45A0-4F42-8737-46345C09C238}").as_deref(),
            Some(BEST_PERFORMANCE)
        );
    }

    #[test]
    fn last_session_restores_the_previous_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let api = Fake::default();
        *api.active.borrow_mut() = Some(BETTER_BATTERY.into());
        let first = acquire(&api, dir.path()).unwrap().unwrap();
        assert_eq!(api.active.borrow().as_deref(), Some(BEST_PERFORMANCE));
        assert_eq!(
            read_marker(dir.path()).unwrap(),
            Some(Some(BETTER_BATTERY.into()))
        );
        let second = acquire(&api, dir.path()).unwrap().unwrap();
        assert_eq!(api.calls.borrow().len(), 1, "set once");
        drop(first);
        assert_eq!(api.active.borrow().as_deref(), Some(BEST_PERFORMANCE));
        drop(second);
        assert_eq!(api.active.borrow().as_deref(), Some(BETTER_BATTERY));
        assert_eq!(read_marker(dir.path()).unwrap(), None);
        assert!(!recover(&api, dir.path()).unwrap());

        // No overlay before: clearing restores "none".
        *api.active.borrow_mut() = None;
        drop(acquire(&api, dir.path()).unwrap());
        assert_eq!(*api.active.borrow(), None);
        assert_eq!(api.calls.borrow().last().cloned(), Some(None));
    }

    #[test]
    fn a_crashed_session_is_undone_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let api = Fake::default();
        let mut lease = acquire(&api, dir.path()).unwrap().unwrap();
        // Simulate a crash: the OS releases the lock and nothing is restored.
        let (file, path) = lease.lease.take().unwrap();
        drop(lease);
        drop(file);
        assert!(path.is_file() && read_marker(dir.path()).unwrap().is_some());
        assert_eq!(api.active.borrow().as_deref(), Some(BEST_PERFORMANCE));
        assert!(recover(&api, dir.path()).unwrap());
        assert_eq!(*api.active.borrow(), None);
        assert!(!path.exists(), "stale lease removed");
        assert_eq!(read_marker(dir.path()).unwrap(), None);

        // A live lease blocks recovery.
        let live = acquire(&api, dir.path()).unwrap().unwrap();
        assert!(!recover(&api, dir.path()).unwrap());
        assert_eq!(api.active.borrow().as_deref(), Some(BEST_PERFORMANCE));
        drop(live);
        assert_eq!(*api.active.borrow(), None);
    }

    #[test]
    fn battery_power_and_user_changes_are_respected() {
        let dir = tempfile::tempdir().unwrap();
        let api = Fake {
            battery: Cell::new(true),
            ..Fake::default()
        };
        assert!(acquire(&api, dir.path()).unwrap().is_none());
        assert!(api.calls.borrow().is_empty());
        assert_eq!(read_marker(dir.path()).unwrap(), None);

        let api = Fake::default();
        let lease = acquire(&api, dir.path()).unwrap().unwrap();
        // The user picks another mode while Android runs: keep it.
        *api.active.borrow_mut() = Some(BETTER_BATTERY.into());
        drop(lease);
        assert_eq!(api.active.borrow().as_deref(), Some(BETTER_BATTERY));
        assert_eq!(read_marker(dir.path()).unwrap(), None);

        // An unreadable marker is reported, not guessed.
        fs::write(dir.path().join(MARKER), "{").unwrap();
        assert!(recover(&api, dir.path()).is_err());
        // A schema 1 marker (revision 2) is the AC value.
        fs::write(
            dir.path().join(MARKER),
            format!(r#"{{"schema":1,"previous":"{BETTER_BATTERY}"}}"#),
        )
        .unwrap();
        assert_eq!(
            read_marker(dir.path()).unwrap(),
            Some(Some(BETTER_BATTERY.into()))
        );
        for unknown in [r#"{"schema":2,"source":"dc"}"#, r#"{"schema":3}"#] {
            fs::write(dir.path().join(MARKER), unknown).unwrap();
            assert!(read_marker(dir.path()).is_err(), "{unknown}");
        }
    }

    #[test]
    fn restore_decision_covers_power_source_marker_and_current_overlay() {
        let previous = [None, Some(BETTER_BATTERY.to_owned())];
        for on_battery in [false, true] {
            for current in [None, Some(BEST_PERFORMANCE), Some(BETTER_BATTERY)] {
                assert_eq!(decide(None, on_battery, current), Restore::Nothing);
                for previous in &previous {
                    let expected = if on_battery {
                        Restore::Wait
                    } else if current == Some(BEST_PERFORMANCE) {
                        Restore::Set(previous.clone())
                    } else {
                        Restore::Forget
                    };
                    assert_eq!(
                        decide(Some(previous.clone()), on_battery, current),
                        expected,
                        "{on_battery} {current:?} {previous:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn unplugging_during_a_session_defers_the_restore_to_ac_power() {
        let dir = tempfile::tempdir().unwrap();
        let api = Fake::default();
        *api.active.borrow_mut() = Some(BETTER_BATTERY.into());
        let lease = acquire(&api, dir.path()).unwrap().unwrap();
        assert!(lease.overlay_active());
        api.battery.set(true);
        drop(lease);
        // Nothing written while on battery; the marker stays.
        assert_eq!(api.calls.borrow().len(), 1);
        assert_eq!(
            read_marker(dir.path()).unwrap(),
            Some(Some(BETTER_BATTERY.into()))
        );
        assert!(!recover(&api, dir.path()).unwrap());
        assert!(acquire(&api, dir.path()).unwrap().is_none());
        assert_eq!(api.calls.borrow().len(), 1);
        // Back on AC: the next desktop start restores the AC value.
        api.battery.set(false);
        assert!(recover(&api, dir.path()).unwrap());
        assert_eq!(api.active.borrow().as_deref(), Some(BETTER_BATTERY));
        assert_eq!(read_marker(dir.path()).unwrap(), None);
    }

    #[test]
    fn a_second_session_reports_the_mode_only_when_it_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let api = Fake::default();
        let first = acquire(&api, dir.path()).unwrap().unwrap();
        assert!(first.overlay_active());
        let second = acquire(&api, dir.path()).unwrap().unwrap();
        assert!(second.overlay_active());
        // The user picked another mode while the first VM ran.
        *api.active.borrow_mut() = Some(BETTER_BATTERY.into());
        let third = acquire(&api, dir.path()).unwrap().unwrap();
        assert!(!third.overlay_active());
        drop((first, second, third));
        assert_eq!(api.active.borrow().as_deref(), Some(BETTER_BATTERY));
        assert_eq!(read_marker(dir.path()).unwrap(), None);
    }
}
