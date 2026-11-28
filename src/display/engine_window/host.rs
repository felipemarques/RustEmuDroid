//! What the panel learns from the client's own window while it holds the
//! engine's surface: its moves (the overlay follows them between two frames
//! of the client), the session being locked or switched away, and the
//! display being turned off (the screen then stops following the engine, and
//! the content check must not take that for a stale surface).

use super::overlay;
use std::cell::Cell;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{
        Power::{
            POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
            UnregisterPowerSettingNotification,
        },
        RemoteDesktop::{
            NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
            WTSUnRegisterSessionNotification,
        },
        SystemServices::GUID_CONSOLE_DISPLAY_STATE,
    },
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            DEVICE_NOTIFY_WINDOW_HANDLE, PBT_POWERSETTINGCHANGE, WM_POWERBROADCAST,
            WM_WINDOWPOSCHANGED, WM_WTSSESSION_CHANGE, WTS_CONSOLE_CONNECT, WTS_CONSOLE_DISCONNECT,
            WTS_REMOTE_CONNECT, WTS_REMOTE_DISCONNECT, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
        },
    },
};

/// Identifies this subclass among the others of the client's window.
const SUBCLASS: usize = 0x5255_5354;

thread_local! {
    /// The session is locked or not shown on this console, and the display is
    /// off (both live on the UI thread with the client's window).
    static AWAY: Cell<(bool, bool)> = const { Cell::new((false, false)) };
}

/// Whether nobody can see the screen now: session locked or disconnected,
/// or display off.
pub(super) fn screen_away() -> bool {
    let (session, display) = AWAY.with(Cell::get);
    session || display
}

/// What a session change means for the screen: `Some(true)` away,
/// `Some(false)` back, `None` nothing.
pub(super) fn session_change(event: u32) -> Option<bool> {
    match event {
        WTS_SESSION_LOCK | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => Some(true),
        WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => Some(false),
        _ => None,
    }
}

fn same_guid(a: &windows_sys::core::GUID, b: &windows_sys::core::GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

unsafe extern "system" fn host_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    match message {
        WM_WINDOWPOSCHANGED => {
            let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
            overlay::follow(window);
            return result;
        }
        WM_WTSSESSION_CHANGE => {
            if let Some(away) = session_change(wparam as u32) {
                AWAY.with(|cell| cell.set((away, cell.get().1)));
            }
        }
        WM_POWERBROADCAST if wparam as u32 == PBT_POWERSETTINGCHANGE && lparam != 0 => {
            // SAFETY: for this event lparam points to a POWERBROADCAST_SETTING.
            let setting = unsafe { &*(lparam as *const POWERBROADCAST_SETTING) };
            if same_guid(&setting.PowerSetting, &GUID_CONSOLE_DISPLAY_STATE)
                && setting.DataLength >= 1
            {
                // 0 off, 1 on, 2 dimmed.
                let off = setting.Data[0] == 0;
                AWAY.with(|cell| cell.set((cell.get().0, off)));
            }
        }
        _ => {}
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

/// The client's window is subclassed, and registered for session and display
/// notifications, while the panel holds the engine's surface.
pub(super) struct HostHook {
    host: isize,
    subclassed: bool,
    session: bool,
    power: isize,
}

impl HostHook {
    pub(super) fn install(host: HWND) -> Self {
        unsafe {
            let subclassed = SetWindowSubclass(host, Some(host_proc), SUBCLASS, 0) != 0;
            let session = WTSRegisterSessionNotification(host, NOTIFY_FOR_THIS_SESSION) != 0;
            let power = RegisterPowerSettingNotification(
                host,
                &GUID_CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
            if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                println!(
                    "engine_child host_hook subclassed={subclassed} session={session} power={}",
                    power != 0
                );
            }
            Self {
                host: host as isize,
                subclassed,
                session,
                power: power as isize,
            }
        }
    }
}

impl Drop for HostHook {
    fn drop(&mut self) {
        let host = self.host as HWND;
        unsafe {
            if self.subclassed {
                RemoveWindowSubclass(host, Some(host_proc), SUBCLASS);
            }
            if self.session {
                WTSUnRegisterSessionNotification(host);
            }
            if self.power != 0 {
                UnregisterPowerSettingNotification(self.power as _);
            }
        }
        AWAY.with(|cell| cell.set((false, false)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_or_disconnected_session_is_away_until_it_comes_back() {
        assert_eq!(session_change(WTS_SESSION_LOCK), Some(true));
        assert_eq!(session_change(WTS_CONSOLE_DISCONNECT), Some(true));
        assert_eq!(session_change(WTS_SESSION_UNLOCK), Some(false));
        assert_eq!(session_change(WTS_REMOTE_CONNECT), Some(false));
        assert_eq!(session_change(0x9), None);
    }

    #[test]
    fn the_client_window_hook_is_removed_cleanly() {
        let host = super::super::child::tests::window(
            windows_sys::Win32::UI::WindowsAndMessaging::WS_OVERLAPPEDWINDOW,
            std::ptr::null_mut(),
        );
        let hook = HostHook::install(host);
        assert!(hook.subclassed);
        assert!(!screen_away());
        drop(hook);
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(host) };
    }
}
