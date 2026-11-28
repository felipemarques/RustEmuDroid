//! The threads that look after the engine's own windows: one keeps the main
//! window off the desktop, out of the taskbar and sized like the panel, and
//! brings the engine's dialogs into view; one pings the main window and,
//! when it stops answering while the client holds its surface, detaches the
//! input queues and gives the surface back, without any drawing of the
//! client being needed. Everything they do to the engine's windows is
//! asynchronous or bounded in time, except giving the surface back, which
//! waits for the engine's thread on the watchdog thread only.

use super::{
    OFF_DESKTOP, child, creation_time, engine_process, on_desktop, render_surface, visible_windows,
    window_pid, window_rect,
};
use std::{
    collections::HashSet,
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
        WindowsAndMessaging::{
            GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetForegroundWindow, GetWindow,
            GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, HWND_TOP, IsIconic,
            IsWindow, SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT, SW_HIDE, SW_SHOWMINNOACTIVE,
            SW_SHOWNOACTIVATE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            SWP_NOZORDER, SendMessageTimeoutW, SetForegroundWindow, SetWindowLongPtrW,
            SetWindowPos, ShowWindowAsync, WM_NULL, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW,
        },
    },
};

/// The engine's window is pinged this often, and waited for this long.
pub(super) const PING_INTERVAL: Duration = Duration::from_millis(250);
/// An engine window that has not answered for this long is taken as hung.
pub(super) const HUNG_AFTER: Duration = Duration::from_secs(3);
/// How often the windows are looked at once the panel copies frames.
const QUIET_INTERVAL: Duration = Duration::from_secs(1);

/// The engine process and whether its window is out of the user's sight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) pid: u32,
    pub(super) created: u64,
    pub(super) hidden: bool,
    /// The engine's main window.
    pub(super) main: isize,
    /// Its render surface while that is still inside it.
    pub(super) surface: Option<isize>,
}

/// What a visible top-level window of the engine is to the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    /// The window that holds (or held) the render surface: off the desktop.
    Main,
    /// A window with a title bar (a message box, a question): the user must
    /// see it, so it is put in front of the client's window.
    Dialog,
    /// Frameless helpers of the main window (its tool bar, its resize
    /// overlay): off the desktop with it.
    Helper,
}

/// The role of a top-level window of the engine from what is known of it.
pub(super) fn classify(
    owned: bool,
    holds_surface: bool,
    known_main: bool,
    title: &str,
    style: u32,
) -> Role {
    if !owned && (holds_surface || known_main || title.starts_with("Android Emulator")) {
        Role::Main
    } else if style & WS_CAPTION == WS_CAPTION {
        Role::Dialog
    } else {
        Role::Helper
    }
}

fn title(window: HWND) -> String {
    let mut text = [0u16; 128];
    let length = unsafe { GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) };
    String::from_utf16_lossy(&text[..length.max(0) as usize])
}

/// Shared by the client's UI thread and the keeper's threads.
pub(super) struct Keeper {
    pub(super) launcher: u32,
    pub(super) stop: AtomicBool,
    /// The client's own window, to give the keyboard back to.
    pub(super) host: AtomicIsize,
    pub(super) found: Mutex<Option<Found>>,
    /// Size wanted for the render surface, taken by the thread.
    pub(super) wanted: Mutex<Option<(i32, i32)>>,
    /// The render surface the client's window holds (set by the UI thread)
    /// and the client's UI thread, whose input queue it joins.
    pub(super) adopted: AtomicIsize,
    pub(super) ui_thread: AtomicU32,
    epoch: Instant,
    /// When the engine's main window last answered a message (milliseconds
    /// after `epoch`, plus one; zero: never).
    pub(super) answered: AtomicU64,
    /// The watchdog found the engine hung and took the surface back from
    /// the client's window.
    pub(super) hung: AtomicBool,
    /// The panel copies frames: the windows are looked at slowly, nothing is
    /// pinged, and the main window is minimised so that the engine does not
    /// keep presenting a picture nobody sees.
    pub(super) quiet: AtomicBool,
    /// Engine dialogs put in front of the client so far.
    pub(super) dialogs: AtomicU32,
}

impl Keeper {
    pub(super) fn new(launcher: u32) -> Self {
        Self {
            launcher,
            stop: AtomicBool::new(false),
            host: AtomicIsize::new(0),
            found: Mutex::new(None),
            wanted: Mutex::new(None),
            adopted: AtomicIsize::new(0),
            ui_thread: AtomicU32::new(0),
            epoch: Instant::now(),
            answered: AtomicU64::new(0),
            hung: AtomicBool::new(false),
            quiet: AtomicBool::new(false),
            dialogs: AtomicU32::new(0),
        }
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64 + 1
    }

    pub(super) fn mark_answered(&self) {
        self.answered.store(self.now(), Ordering::Release);
    }

    /// Whether the engine's window answered within `within`; `None` before
    /// its first answer.
    pub(super) fn answering(&self, within: Duration) -> Option<bool> {
        let answered = self.answered.load(Ordering::Acquire);
        (answered != 0).then(|| self.now().saturating_sub(answered) <= within.as_millis() as u64)
    }

    /// The watchdog thread: pings the engine's main window and handles a
    /// hang whether or not the client draws its panel.
    pub(super) fn watch(&self) {
        while !self.stop.load(Ordering::Acquire) {
            if self.quiet.load(Ordering::Acquire) {
                thread::sleep(QUIET_INTERVAL);
                continue;
            }
            let main = self.found.lock().unwrap().map_or(0, |found| found.main);
            if main == 0 || unsafe { IsWindow(main as HWND) } == 0 {
                // No window to ask: no evidence of a hang either.
                self.mark_answered();
            } else {
                let mut result = 0;
                let answered = unsafe {
                    SendMessageTimeoutW(
                        main as HWND,
                        WM_NULL,
                        0,
                        0,
                        SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                        PING_INTERVAL.as_millis() as u32,
                        &mut result,
                    )
                } != 0;
                if answered {
                    self.mark_answered();
                }
            }
            if self.adopted.load(Ordering::Acquire) != 0
                && !self.hung.load(Ordering::Acquire)
                && self.answering(HUNG_AFTER) == Some(false)
            {
                self.take_back(main);
            }
            thread::sleep(PING_INTERVAL);
        }
    }

    /// The engine stopped answering while its surface is in the client's
    /// window: the client's input queue is detached from the engine's thread
    /// at once, then the surface goes back to the engine's window (that waits
    /// for the engine's thread, here only).
    fn take_back(&self, main: isize) {
        self.hung.store(true, Ordering::Release);
        let surface = self.adopted.load(Ordering::Acquire);
        let engine = unsafe { GetWindowThreadProcessId(surface as HWND, ptr::null_mut()) };
        let ui = self.ui_thread.load(Ordering::Acquire);
        let detached = engine != 0 && ui != 0 && unsafe { AttachThreadInput(ui, engine, 0) } != 0;
        if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
            println!("engine_watchdog hung input_detached={detached}");
        }
        child::give_back(surface, main);
        self.adopted.store(0, Ordering::Release);
        if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
            println!("engine_watchdog surface_given_back");
        }
    }

    pub(super) fn run(&self) {
        // Window coordinates of another process are physical pixels.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let mut engine: Option<(u32, u64)> = None;
        let mut first_seen: Option<Instant> = None;
        let mut restyled = 0;
        let mut main_window: isize = 0;
        let mut minimised = false;
        // The windows asked to move lately: the engine's thread may take a while.
        let mut moved: Vec<(isize, Instant)> = Vec::new();
        let mut dialogs: HashSet<isize> = HashSet::new();
        let mut next_lookup = Instant::now();
        while !self.stop.load(Ordering::Acquire) {
            if engine.is_none() && Instant::now() >= next_lookup {
                next_lookup = Instant::now() + Duration::from_millis(100);
                engine = engine_process(self.launcher)
                    .and_then(|pid| creation_time(pid).map(|created| (pid, created)));
            }
            let quiet = self.quiet.load(Ordering::Acquire);
            let mut hidden = false;
            if let Some((pid, created)) = engine {
                if creation_time(pid) != Some(created) {
                    // The engine exited.
                    engine = None;
                    *self.found.lock().unwrap() = None;
                    continue;
                }
                let windows = visible_windows(pid);
                if !windows.is_empty() {
                    first_seen.get_or_insert_with(Instant::now);
                }
                // A main window hidden for a moment (its restyling) stays the main one.
                if main_window != 0 && unsafe { IsWindow(main_window as HWND) } == 0 {
                    main_window = 0;
                }
                let mut inside = None;
                moved.retain(|(_, at)| at.elapsed() < Duration::from_millis(100));
                for window in windows.iter().map(|window| *window as HWND) {
                    let surface = render_surface(window);
                    let owned = !unsafe { GetWindow(window, GW_OWNER) }.is_null();
                    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32;
                    let role = classify(
                        owned,
                        surface.is_some(),
                        window as isize == main_window,
                        &title(window),
                        style,
                    );
                    if role == Role::Dialog {
                        if dialogs.insert(window as isize) {
                            self.show_dialog(window);
                        }
                        continue;
                    }
                    if window_rect(window).is_some_and(on_desktop)
                        && unsafe { IsIconic(window) } == 0
                        && !moved.iter().any(|(which, _)| *which == window as isize)
                    {
                        unsafe {
                            SetWindowPos(
                                window,
                                ptr::null_mut(),
                                OFF_DESKTOP.0,
                                OFF_DESKTOP.1,
                                0,
                                0,
                                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                            )
                        };
                        moved.push((window as isize, Instant::now()));
                        if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                            println!(
                                "engine_window moved_off_desktop after_ms={}",
                                first_seen.map_or(0, |seen| seen.elapsed().as_millis())
                            );
                        }
                    }
                    if role == Role::Main {
                        main_window = window as isize;
                        inside = surface;
                        // An engine that puts its styles back keeps its button.
                        if restyled < 3 && self.keep_out_of_the_taskbar(window) {
                            restyled += 1;
                        }
                        if !quiet {
                            self.resize(window, surface);
                        }
                        hidden = window_rect(window).is_some_and(|rect| !on_desktop(rect));
                    }
                }
                dialogs.retain(|window| windows.contains(window));
                // Copying frames: the engine's own picture is not wanted. A
                // minimised window is not presented to; it comes back when
                // the panel tries the engine's surface again.
                if main_window != 0 && quiet != minimised {
                    let command = if quiet {
                        SW_SHOWMINNOACTIVE
                    } else {
                        SW_SHOWNOACTIVATE
                    };
                    unsafe { ShowWindowAsync(main_window as HWND, command) };
                    minimised = quiet;
                }
                if dialogs.is_empty() {
                    self.give_the_keyboard_back(pid);
                }
                *self.found.lock().unwrap() = Some(Found {
                    pid,
                    created,
                    hidden: hidden || minimised,
                    main: main_window,
                    surface: inside,
                });
            }
            // The first seconds of the engine's window are watched closely.
            let settled =
                hidden && first_seen.is_some_and(|seen| seen.elapsed() > Duration::from_secs(5));
            thread::sleep(if quiet {
                QUIET_INTERVAL
            } else if settled {
                super::CHECK_INTERVAL
            } else {
                Duration::from_millis(5)
            });
        }
    }

    /// Puts an engine dialog in the middle of the client's window, in front,
    /// with the keyboard: the engine may wait for its answer.
    fn show_dialog(&self, dialog: HWND) {
        self.dialogs.fetch_add(1, Ordering::AcqRel);
        let host = self.host.load(Ordering::Acquire) as HWND;
        let (Some(dialog_rect), Some(host_rect)) = (window_rect(dialog), window_rect(host)) else {
            return;
        };
        let (width, height) = (
            dialog_rect.right - dialog_rect.left,
            dialog_rect.bottom - dialog_rect.top,
        );
        let x = (host_rect.left + host_rect.right - width) / 2;
        let y = (host_rect.top + host_rect.bottom - height) / 2;
        unsafe {
            SetWindowPos(
                dialog,
                HWND_TOP,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_ASYNCWINDOWPOS,
            );
            SetForegroundWindow(dialog);
        }
        if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
            println!("engine_window dialog_shown title={:?}", title(dialog));
        }
    }

    /// No taskbar button, no Alt+Tab entry, never activated.
    fn keep_out_of_the_taskbar(&self, window: HWND) -> bool {
        let style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
        let wanted =
            (style | (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize) & !(WS_EX_APPWINDOW as isize);
        // Changing a style waits for the engine's thread: only while it answers.
        if style == wanted || self.answering(Duration::from_millis(500)) != Some(true) {
            return false;
        }
        // The shell only drops the button of a window that is shown again.
        unsafe {
            ShowWindowAsync(window, SW_HIDE);
            SetWindowLongPtrW(window, GWL_EXSTYLE, wanted);
            ShowWindowAsync(window, SW_SHOWNOACTIVATE);
        }
        true
    }

    fn resize(&self, window: HWND, surface: Option<isize>) {
        let Some((width, height)) = self.wanted.lock().unwrap().take() else {
            return;
        };
        // The surface is held by the client's window.
        let adopted = self.adopted.load(Ordering::Acquire);
        let surface = surface.or_else(|| {
            (adopted != 0 && unsafe { IsWindow(adopted as HWND) } != 0).then_some(adopted)
        });
        let (Some(outer), Some(surface)) = (window_rect(window), surface) else {
            return;
        };
        let mut inner = RECT::default();
        if unsafe { GetClientRect(surface as HWND, &mut inner) } == 0 {
            return;
        }
        // The engine keeps the aspect of the Android screen and scales its
        // render surface to the window.
        let extra_width = (outer.right - outer.left) - (inner.right - inner.left);
        let extra_height = (outer.bottom - outer.top) - (inner.bottom - inner.top);
        unsafe {
            SetWindowPos(
                window,
                ptr::null_mut(),
                0,
                0,
                width + extra_width.max(0),
                height + extra_height.max(0),
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        };
    }

    /// A new engine window takes the keyboard; the client gets it back.
    fn give_the_keyboard_back(&self, pid: u32) {
        let host = self.host.load(Ordering::Acquire) as HWND;
        let foreground = unsafe { GetForegroundWindow() };
        if host.is_null() || foreground.is_null() || window_pid(foreground) != pid {
            return;
        }
        unsafe {
            if IsWindow(host) == 0 || IsIconic(host) != 0 {
                return;
            }
            if SetForegroundWindow(host) == 0 {
                let owner = GetWindowThreadProcessId(foreground, ptr::null_mut());
                let this = GetCurrentThreadId();
                if owner != 0 && AttachThreadInput(this, owner, 1) != 0 {
                    SetForegroundWindow(host);
                    AttachThreadInput(this, owner, 0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WS_POPUP, WS_SYSMENU};

    #[test]
    fn only_the_main_window_and_its_frameless_helpers_leave_the_desktop() {
        let frameless = WS_POPUP;
        let titled = WS_CAPTION | WS_SYSMENU;
        let title = "Android Emulator - RustEmuDroid:5556";
        assert_eq!(classify(false, true, false, "", frameless), Role::Main);
        assert_eq!(classify(false, false, true, "", frameless), Role::Main);
        assert_eq!(classify(false, false, false, title, frameless), Role::Main);
        // The tool bar and the resize overlay: owned and frameless.
        assert_eq!(
            classify(true, false, false, "Emulator", frameless),
            Role::Helper
        );
        // A message box or question: titled, owned or not.
        assert_eq!(
            classify(true, false, false, "Emulator", titled),
            Role::Dialog
        );
        assert_eq!(
            classify(false, false, false, "Warning", titled),
            Role::Dialog
        );
        assert_eq!(classify(true, false, false, title, titled), Role::Dialog);
    }

    #[test]
    fn an_engine_window_is_judged_by_its_last_answer() {
        let keeper = Keeper::new(0);
        assert_eq!(keeper.answering(HUNG_AFTER), None);
        keeper.mark_answered();
        assert_eq!(keeper.answering(HUNG_AFTER), Some(true));
        let keeper = Keeper {
            epoch: Instant::now() - Duration::from_secs(60),
            ..Keeper::new(0)
        };
        keeper.answered.store(
            keeper.now() - HUNG_AFTER.as_millis() as u64 - 100,
            Ordering::Release,
        );
        assert_eq!(keeper.answering(HUNG_AFTER), Some(false));
        assert_eq!(keeper.answering(HUNG_AFTER * 2), Some(true));
    }
}
