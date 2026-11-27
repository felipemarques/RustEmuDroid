//! Windows: the engine's own window shown through the desktop compositor.
//!
//! The engine runs with its real window, which presents with its own
//! swapchain, kept off the desktop. A DWM thumbnail of that window is drawn in
//! a backdrop window that sits right behind the client's window, and the
//! client leaves its panel transparent over it. No pixel of the game passes
//! through this process, and everything the client paints over the game
//! (control markers, the controls editor, notices, menus) stays on top.
//!
//! Measured on Windows 11 (RTX 5070 Ti, Android Emulator with the native
//! Vulkan swapchain): a thumbnail of the engine's window stays live while the
//! window is off the desktop, without a taskbar button, resized from outside
//! and rotated; it goes stale when the window is minimised or hidden. The
//! stock `-qt-hide-window` creates no render surface at all, and only the
//! engine's own process may cloak its window (a cloaked window of one's own
//! stays live), so with stock flags the window is moved off the desktop.

use crate::bluestacks::{
    player_process::process_creation_time,
    preview::{PreviewError, PreviewRect, ThumbnailPreview},
};
use crate::ui::i18n::Locale;
use std::{
    cell::Cell,
    os::windows::io::{FromRawHandle, OwnedHandle},
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND, DwmEnableBlurBehindWindow},
        Gdi::{
            BLACK_BRUSH, ClientToScreen, CreateRectRgn, DeleteObject, GetMonitorInfoW,
            GetStockObject, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect,
        },
    },
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        LibraryLoader::GetModuleHandleW,
        Threading::{
            AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
        WindowsAndMessaging::{
            CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, EnumChildWindows,
            EnumWindows, GW_HWNDNEXT, GW_OWNER, GWL_EXSTYLE, GWLP_WNDPROC, GetClassNameW,
            GetClientRect, GetForegroundWindow, GetSystemMetrics, GetWindow, GetWindowLongPtrW,
            GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
            MA_NOACTIVATE, RegisterClassW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
            SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetForegroundWindow,
            SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOWPOS, WM_MOUSEACTIVATE,
            WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING, WNDCLASSW, WNDPROC, WS_DISABLED,
            WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

/// Where the engine's windows are kept: far outside any monitor arrangement
/// and inside the 16-bit range of window coordinates.
const OFF_DESKTOP: (i32, i32) = (-25_000, -25_000);
/// The engine's render surface must show up within this time once the panel
/// has a picture from the frame stream.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(30);
/// A thumbnail that was live and broke (rotation recreates the surface) gets
/// this long to come back.
const RECOVER_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_millis(200);
const RESIZE_SETTLE: Duration = Duration::from_millis(250);
const ENGINE_EXE: &str = "qemu-system-x86_64.exe";

/// Why the panel shows copied frames although the engine's window was wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// No engine window with a render surface appeared in time.
    NoWindow,
    /// The window was shown and then stopped being usable; the reason in the
    /// thumbnail code's own words.
    Lost(String),
    /// Windows refused a call: its name and code, verbatim.
    System(String),
    /// The engine's picture kept another shape than the Android screen.
    OtherShape,
}

/// What the panel shows, for the boot details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelPath {
    /// The engine's own window, composed by Windows.
    EngineWindow,
    /// Frames copied from the engine, and why.
    Frames(Fault),
}

impl PanelPath {
    pub(crate) fn describe(&self, locale: Locale) -> String {
        let fault = match self {
            Self::EngineWindow => {
                return locale
                    .pick(
                        "Painel: a janela do próprio motor, mostrada pela composição do Windows; nenhum quadro é copiado para o cliente.",
                        "Panel: the engine's own window, shown through the Windows compositor; no frame is copied into the client.",
                        "Panel: la ventana del propio motor, mostrada por el compositor de Windows; no se copia ningún fotograma al cliente.",
                    )
                    .into();
            }
            Self::Frames(fault) => fault,
        };
        let lead = locale.pick(
            "Painel: quadros copiados do motor; a janela do motor não foi usada porque",
            "Panel: frames copied from the engine; the engine's window was not used because",
            "Panel: fotogramas copiados del motor; no se usó la ventana del motor porque",
        );
        let seconds = APPEAR_TIMEOUT.as_secs();
        let reason = match fault {
            Fault::NoWindow => match locale {
                Locale::PtBr => {
                    format!("ela não apareceu com uma superfície de imagem em {seconds} segundos.")
                }
                Locale::EnUs => {
                    format!("it did not appear with a render surface within {seconds} seconds.")
                }
                Locale::EsEs => {
                    format!("no apareció con una superficie de imagen en {seconds} segundos.")
                }
            },
            Fault::Lost(detail) => format!(
                "{} “{detail}”",
                locale.pick(
                    "ela deixou de estar disponível; detalhe:",
                    "it stopped being available; detail:",
                    "dejó de estar disponible; detalle:",
                )
            ),
            Fault::System(detail) => format!(
                "{} “{detail}”",
                locale.pick(
                    "o Windows recusou uma chamada; mensagem do sistema:",
                    "Windows refused a call; the system's message:",
                    "Windows rechazó una llamada; mensaje del sistema:",
                )
            ),
            Fault::OtherShape => locale
                .pick(
                    "a imagem dela ficou com um formato diferente do da tela do Android.",
                    "its picture kept a different shape from the Android screen.",
                    "su imagen quedó con una forma distinta de la pantalla de Android.",
                )
                .to_string(),
        };
        format!("{lead} {reason}")
    }
}

/// The thumbnail code's words for the engine (they name BlueStacks for the
/// player it was written for); system errors stay verbatim.
fn detail(error: &PreviewError) -> String {
    match error {
        PreviewError::NoSource => "The engine window has no visible render surface".into(),
        PreviewError::AmbiguousSource => {
            "The engine has more than one visible render surface".into()
        }
        PreviewError::SourceUnavailable => {
            "The engine window is minimised, hidden or cloaked".into()
        }
        PreviewError::InvalidCrop => "The render surface is outside the engine window".into(),
        PreviewError::InvalidPlayer | PreviewError::PlayerIdentityChanged => {
            "The process is not the engine this session started".into()
        }
        PreviewError::PlayerUnavailable => "The engine process has exited".into(),
        PreviewError::EmptySlot => "The panel has no client pixels".into(),
        PreviewError::InvalidDestination => "The panel's window is unavailable".into(),
        other => other.to_string(),
    }
}

fn window_pid(window: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    pid
}

fn window_rect(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    (unsafe { GetWindowRect(window, &mut rect) } != 0).then_some(rect)
}

fn on_desktop(rect: RECT) -> bool {
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    rect.left < left + width && rect.right > left && rect.top < top + height && rect.bottom > top
}

/// The emulator process started by the launcher `launcher`: the QEMU binary
/// whose parent chain reaches it.
fn engine_process(launcher: u32) -> Option<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut processes = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        processes.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..length]),
        ));
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    let parent = |pid: u32| {
        processes
            .iter()
            .find(|(candidate, ..)| *candidate == pid)
            .map(|(_, parent, _)| *parent)
    };
    let descends = |mut pid: u32| {
        for _ in 0..4 {
            match parent(pid) {
                Some(next) if next == launcher => return true,
                Some(next) if next != 0 && next != pid => pid = next,
                _ => return false,
            }
        }
        false
    };
    let mut engines = processes
        .iter()
        .filter(|(pid, _, name)| name.eq_ignore_ascii_case(ENGINE_EXE) && descends(*pid));
    let engine = engines.next()?.0;
    // Two candidates would mean the launcher is not the one this session started.
    engines.next().is_none().then_some(engine)
}

fn creation_time(pid: u32) -> Option<u64> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let process = unsafe { OwnedHandle::from_raw_handle(handle) };
    process_creation_time(&process)
        .ok()
        .filter(|time| *time != 0)
}

/// Visible top-level windows of one process.
fn visible_windows(pid: u32) -> Vec<isize> {
    struct Search {
        pid: u32,
        found: Vec<isize>,
    }
    unsafe extern "system" fn each(window: HWND, context: LPARAM) -> i32 {
        // Enumeration is synchronous and context points to our live Search.
        let search = unsafe { &mut *(context as *mut Search) };
        if window_pid(window) == search.pid && unsafe { IsWindowVisible(window) } != 0 {
            search.found.push(window as isize);
        }
        1
    }
    let mut search = Search {
        pid,
        found: Vec::new(),
    };
    unsafe { EnumWindows(Some(each), (&mut search as *mut Search) as LPARAM) };
    search.found
}

/// The render child of an engine window, when it has one.
fn render_surface(window: HWND) -> Option<isize> {
    unsafe extern "system" fn each(window: HWND, context: LPARAM) -> i32 {
        let found = unsafe { &mut *(context as *mut Option<isize>) };
        let mut class = [0u16; 16];
        let length =
            unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) } as usize;
        if class
            .get(..length)
            .is_some_and(|name| name.iter().copied().eq("subWin".encode_utf16()))
        {
            *found = Some(window as isize);
            return 0;
        }
        1
    }
    let mut found: Option<isize> = None;
    unsafe {
        EnumChildWindows(
            window,
            Some(each),
            (&mut found as *mut Option<isize>) as LPARAM,
        )
    };
    found
}

/// The engine process and whether its window is out of the user's sight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Found {
    pid: u32,
    created: u64,
    hidden: bool,
}

/// Shared with the thread that keeps the engine's windows off the desktop.
struct Keeper {
    launcher: u32,
    stop: AtomicBool,
    /// The client's own window, to give the keyboard back to.
    host: AtomicIsize,
    found: Mutex<Option<Found>>,
    /// Size wanted for the render surface, taken by the thread.
    wanted: Mutex<Option<(i32, i32)>>,
}

impl Keeper {
    fn run(&self) {
        // Window coordinates of another process are physical pixels.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let mut engine: Option<(u32, u64)> = None;
        let mut first_seen: Option<Instant> = None;
        let mut restyled = 0;
        let mut next_lookup = Instant::now();
        while !self.stop.load(Ordering::Acquire) {
            if engine.is_none() && Instant::now() >= next_lookup {
                next_lookup = Instant::now() + Duration::from_millis(100);
                engine = engine_process(self.launcher)
                    .and_then(|pid| creation_time(pid).map(|created| (pid, created)));
            }
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
                for window in windows.iter().map(|window| *window as HWND) {
                    if window_rect(window).is_some_and(on_desktop) {
                        unsafe {
                            SetWindowPos(
                                window,
                                ptr::null_mut(),
                                OFF_DESKTOP.0,
                                OFF_DESKTOP.1,
                                0,
                                0,
                                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                            )
                        };
                    }
                    let main = unsafe { GetWindow(window, GW_OWNER) }.is_null()
                        && render_surface(window).is_some();
                    if main {
                        // An engine that puts its styles back keeps its button.
                        if restyled < 3 && self.keep_out_of_the_taskbar(window) {
                            restyled += 1;
                        }
                        self.resize(window);
                        hidden = window_rect(window).is_some_and(|rect| !on_desktop(rect));
                    }
                }
                self.give_the_keyboard_back(pid);
                *self.found.lock().unwrap() = Some(Found {
                    pid,
                    created,
                    hidden,
                });
            }
            // The engine recentres its window once right after creating it, so
            // the first seconds are watched closely.
            let settled =
                hidden && first_seen.is_some_and(|seen| seen.elapsed() > Duration::from_secs(5));
            thread::sleep(if settled {
                CHECK_INTERVAL
            } else {
                Duration::from_millis(10)
            });
        }
    }

    /// No taskbar button, no Alt+Tab entry, never activated.
    fn keep_out_of_the_taskbar(&self, window: HWND) -> bool {
        let style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
        let wanted =
            (style | (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize) & !(WS_EX_APPWINDOW as isize);
        if style == wanted {
            return false;
        }
        // The shell only drops the button of a window that is shown again.
        unsafe {
            ShowWindow(window, SW_HIDE);
            SetWindowLongPtrW(window, GWL_EXSTYLE, wanted);
            ShowWindow(window, SW_SHOWNOACTIVATE);
        }
        true
    }

    fn resize(&self, window: HWND) {
        let Some((width, height)) = self.wanted.lock().unwrap().take() else {
            return;
        };
        let (Some(outer), Some(surface)) = (window_rect(window), render_surface(window)) else {
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
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
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

thread_local! {
    /// The backdrop that follows the client's window (both live on the UI thread).
    static BACKDROP: Cell<isize> = const { Cell::new(0) };
    /// The window procedure of the client's window before `host_proc`.
    static HOST_PROC: Cell<isize> = const { Cell::new(0) };
}

/// Puts the backdrop exactly under the client area of `host`, right behind it.
fn follow(host: HWND) {
    let backdrop = BACKDROP.with(Cell::get) as HWND;
    if backdrop.is_null() {
        return;
    }
    unsafe {
        if IsIconic(host) != 0 || IsWindowVisible(host) == 0 {
            ShowWindow(backdrop, SW_HIDE);
            return;
        }
        let mut client = RECT::default();
        let mut origin = POINT { x: 0, y: 0 };
        if GetClientRect(host, &mut client) == 0 || ClientToScreen(host, &mut origin) == 0 {
            return;
        }
        let wanted = RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        };
        let placed = window_rect(backdrop).is_some_and(|rect| {
            (rect.left, rect.top, rect.right, rect.bottom)
                == (wanted.left, wanted.top, wanted.right, wanted.bottom)
        });
        if placed && IsWindowVisible(backdrop) != 0 && GetWindow(host, GW_HWNDNEXT) == backdrop {
            return;
        }
        SetWindowPos(
            backdrop,
            host,
            wanted.left,
            wanted.top,
            wanted.right - wanted.left,
            wanted.bottom - wanted.top,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn host_proc_address() -> isize {
    host_proc as *const () as isize
}

/// Keeps the backdrop glued to the client's window while Windows moves,
/// resizes, minimises or restacks it, between two frames of the client.
unsafe extern "system" fn host_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let previous = HOST_PROC.with(Cell::get);
    let result = if previous == 0 {
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    } else {
        // SAFETY: the value was returned by SetWindowLongPtrW(GWLP_WNDPROC) for this window.
        let previous: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(previous) };
        unsafe { CallWindowProcW(previous, window, message, wparam, lparam) }
    };
    if message == WM_WINDOWPOSCHANGING && lparam != 0 {
        // SAFETY: for this message lparam points to the WINDOWPOS being applied.
        keep_off_full_cover(window, unsafe { &mut *(lparam as *mut WINDOWPOS) });
    }
    if message == WM_WINDOWPOSCHANGED {
        follow(window);
    }
    result
}

/// A window that covers a whole monitor is presented by the graphics driver
/// past the compositor (independent flip): the alpha of the panel stops
/// counting and the engine's window behind it is hidden, and for the OpenGL
/// context of this client that stays so even after the window is made
/// smaller again. So the client's window stays one pixel short of covering a
/// monitor (fullscreen keeps a one-pixel line at the bottom).
fn keep_off_full_cover(window: HWND, position: &mut WINDOWPOS) {
    if position.flags & SWP_NOSIZE != 0 {
        return;
    }
    let (left, top) = if position.flags & SWP_NOMOVE != 0 {
        match window_rect(window) {
            Some(rect) => (rect.left, rect.top),
            None => return,
        }
    } else {
        (position.x, position.y)
    };
    let wanted = RECT {
        left,
        top,
        right: left + position.cx,
        bottom: top + position.cy,
    };
    if let Some(bottom) = short_of_full_cover(wanted, monitor_of(wanted)) {
        position.cy = bottom - top;
    }
}

fn monitor_of(rect: RECT) -> Option<RECT> {
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some(info.rcMonitor)
}

/// The bottom edge that keeps `rect` from covering all of `monitor`.
fn short_of_full_cover(rect: RECT, monitor: Option<RECT>) -> Option<i32> {
    let monitor = monitor?;
    (rect.left <= monitor.left
        && rect.top <= monitor.top
        && rect.right >= monitor.right
        && rect.bottom >= monitor.bottom
        && monitor.bottom - 1 > rect.top)
        .then_some(monitor.bottom - 1)
}

/// The client's window procedure is `host_proc` while the engine's window
/// may be shown behind the panel.
struct HostHook {
    host: isize,
}

impl HostHook {
    fn install(host: HWND) -> Self {
        unsafe {
            let previous = SetWindowLongPtrW(host, GWLP_WNDPROC, host_proc_address());
            HOST_PROC.with(|cell| cell.set(previous));
            // Already covering a monitor (fullscreen before Android started):
            // made one pixel shorter now, through host_proc.
            if let Some(rect) = window_rect(host)
                && short_of_full_cover(rect, monitor_of(rect)).is_some()
            {
                SetWindowPos(
                    host,
                    ptr::null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        Self {
            host: host as isize,
        }
    }
}

impl Drop for HostHook {
    fn drop(&mut self) {
        let host = self.host as HWND;
        unsafe {
            let previous = HOST_PROC.with(Cell::get);
            if IsWindow(host) != 0
                && previous != 0
                && GetWindowLongPtrW(host, GWLP_WNDPROC) == host_proc_address()
            {
                SetWindowLongPtrW(host, GWLP_WNDPROC, previous);
            }
            HOST_PROC.with(|cell| cell.set(0));
        }
    }
}

unsafe extern "system" fn backdrop_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_MOUSEACTIVATE {
        return MA_NOACTIVATE as LRESULT;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// The black window behind the client's window that carries the thumbnail.
struct Backdrop {
    window: isize,
    host: isize,
}

impl Backdrop {
    fn create(host: HWND) -> Result<Self, Fault> {
        let system =
            |api: &str| Fault::System(format!("{api}: {}", std::io::Error::last_os_error()));
        let class: Vec<u16> = "RustEmuDroidEnginePanel\0".encode_utf16().collect();
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let definition = WNDCLASSW {
                lpfnWndProc: Some(backdrop_proc),
                hInstance: instance,
                hbrBackground: GetStockObject(BLACK_BRUSH),
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // A second session in this process finds the class registered.
            RegisterClassW(&definition);
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                ptr::null(),
                WS_POPUP | WS_DISABLED,
                0,
                0,
                1,
                1,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if window.is_null() {
                return Err(system("CreateWindowExW"));
            }
            let backdrop = Self {
                window: window as isize,
                host: host as isize,
            };
            // The alpha of the client's picture now reaches the compositor:
            // where the panel is left unpainted the backdrop shows through.
            let region = CreateRectRgn(0, 0, -1, -1);
            let blur = DWM_BLURBEHIND {
                dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
                fEnable: 1,
                hRgnBlur: region,
                fTransitionOnMaximized: 0,
            };
            let code = DwmEnableBlurBehindWindow(host, &blur);
            DeleteObject(region);
            if code < 0 {
                return Err(Fault::System(format!(
                    "DwmEnableBlurBehindWindow: HRESULT 0x{:08X}",
                    code as u32
                )));
            }
            BACKDROP.with(|cell| cell.set(backdrop.window));
            follow(host);
            Ok(backdrop)
        }
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        let host = self.host as HWND;
        unsafe {
            if IsWindow(host) != 0 {
                let blur = DWM_BLURBEHIND {
                    dwFlags: DWM_BB_ENABLE,
                    fEnable: 0,
                    hRgnBlur: ptr::null_mut(),
                    fTransitionOnMaximized: 0,
                };
                DwmEnableBlurBehindWindow(host, &blur);
            }
            BACKDROP.with(|cell| cell.set(0));
            DestroyWindow(self.window as HWND);
        }
    }
}

/// What the panel may do in this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The engine's window is behind `0` (physical client pixels): leave it unpainted.
    Through(PreviewRect),
    /// Not yet, or not at the moment: paint the copied frames.
    Frames,
    /// The client's window is minimised: nothing is seen and nothing is needed.
    Unseen,
}

pub(crate) struct EngineWindow {
    keeper: Arc<Keeper>,
    thread: Option<thread::JoinHandle<()>>,
    backdrop: Option<Backdrop>,
    hook: Option<HostHook>,
    preview: Option<ThumbnailPreview>,
    /// The slot last given to the thumbnail, its answer and when.
    layout: Option<(PreviewRect, PreviewRect, Instant)>,
    /// Since when the panel has waited for a usable engine window.
    waiting: Option<Instant>,
    retry_at: Option<Instant>,
    /// What kept the engine's window from being shown, should it last.
    pending: Option<Fault>,
    was_live: bool,
    fault: Option<Fault>,
    /// The slot the render surface should match, since when, and what was asked.
    resize: Option<(PreviewRect, Instant)>,
    resize_asked: Option<((i32, i32), u8)>,
    reported: Option<PanelPath>,
    report_pending: Option<PanelPath>,
    trace: Option<(PreviewRect, (i32, i32))>,
}

impl EngineWindow {
    /// `launcher` is the emulator launcher this session started.
    pub(crate) fn watch(launcher: u32) -> Self {
        let keeper = Arc::new(Keeper {
            launcher,
            stop: AtomicBool::new(false),
            host: AtomicIsize::new(0),
            found: Mutex::new(None),
            wanted: Mutex::new(None),
        });
        let worker = keeper.clone();
        let thread = thread::Builder::new()
            .name("engine window".into())
            .spawn(move || worker.run())
            .ok();
        Self {
            keeper,
            thread,
            backdrop: None,
            hook: None,
            preview: None,
            layout: None,
            waiting: None,
            retry_at: None,
            pending: None,
            was_live: false,
            fault: None,
            resize: None,
            resize_asked: None,
            reported: None,
            report_pending: None,
            trace: None,
        }
    }

    /// The client's own window, as soon as it is known.
    pub(crate) fn set_host(&mut self, host: isize) {
        self.keeper.host.store(host, Ordering::Release);
        // As early as possible: a client window that covered a monitor once
        // could not show the engine's window again.
        if host != 0 && self.hook.is_none() && self.fault.is_none() {
            self.hook = Some(HostHook::install(host as HWND));
        }
    }

    /// Still trying to show the engine's window (it has not been given up).
    pub(crate) fn active(&self) -> bool {
        self.fault.is_none()
    }

    /// The path the panel is on, once per change.
    pub(crate) fn take_path_change(&mut self) -> Option<PanelPath> {
        self.report_pending.take()
    }

    fn report(&mut self, path: PanelPath) {
        if self.reported.as_ref() != Some(&path) {
            if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                println!("panel_path={path:?}");
            }
            self.reported = Some(path.clone());
            self.report_pending = Some(path);
        }
    }

    fn fail(&mut self, fault: Fault) -> Shown {
        self.preview = None;
        self.layout = None;
        self.backdrop = None;
        self.hook = None;
        self.report(PanelPath::Frames(fault.clone()));
        self.fault = Some(fault);
        Shown::Frames
    }

    /// Stops showing the engine's window for now (the panel is not drawn).
    pub(crate) fn hide(&mut self) {
        if let Some(preview) = &mut self.preview {
            let _ = preview.hide();
        }
        self.layout = None;
    }

    /// Places the engine's picture behind `slot` (the fitted Android picture
    /// in physical client pixels of the client's window) and says whether the
    /// panel can be left transparent over it.
    pub(crate) fn show(&mut self, slot: Option<PreviewRect>, now: Instant) -> Shown {
        if self.fault.is_some() {
            return Shown::Frames;
        }
        let host = self.keeper.host.load(Ordering::Acquire);
        if host != 0 && unsafe { IsIconic(host as HWND) } != 0 {
            self.hide();
            self.waiting = Some(now);
            return Shown::Unseen;
        }
        let found = *self.keeper.found.lock().unwrap();
        let waiting = *self.waiting.get_or_insert(now);
        let (Some(slot), Some(found), true) = (slot, found.filter(|found| found.hidden), host != 0)
        else {
            self.hide();
            return self.wait(waiting, now);
        };
        if self.backdrop.is_none() {
            match Backdrop::create(host as HWND) {
                Ok(backdrop) => self.backdrop = Some(backdrop),
                Err(fault) => return self.fail(fault),
            }
        }
        let backdrop = self.backdrop.as_ref().unwrap().window;
        follow(host as HWND);
        if self.preview.is_none() {
            if self.retry_at.is_some_and(|at| now < at) {
                return self.wait(waiting, now);
            }
            self.retry_at = Some(now + Duration::from_millis(500));
            match ThumbnailPreview::attach_qt_emulator(backdrop as HWND, found.pid, found.created) {
                Ok(preview) => self.preview = Some(preview),
                Err(error) => return self.unavailable(error, waiting, now),
            }
        }
        // DWM updates the picture by itself. The source geometry is checked a
        // few times per second and at once when the panel changes.
        let fitted = match self.layout {
            Some((previous, fitted, checked))
                if previous == slot && now.duration_since(checked) < CHECK_INTERVAL =>
            {
                fitted
            }
            _ => match self.preview.as_mut().unwrap().position(slot) {
                Ok(fitted) => {
                    self.layout = Some((slot, fitted, now));
                    fitted
                }
                Err(error) => {
                    self.preview = None;
                    self.layout = None;
                    return self.unavailable(error, waiting, now);
                }
            },
        };
        // A picture of another shape means Android is turning: the frames
        // bring the new geometry.
        let close = |a: i32, b: i32| (a - b).abs() <= 2 + b / 50;
        if !close(fitted.right - fitted.left, slot.right - slot.left)
            || !close(fitted.bottom - fitted.top, slot.bottom - slot.top)
        {
            self.pending = Some(Fault::OtherShape);
            return self.wait(waiting, now);
        }
        self.waiting = Some(now);
        self.was_live = true;
        self.pending = None;
        self.match_surface(slot, now);
        self.report(PanelPath::EngineWindow);
        Shown::Through(fitted)
    }

    fn wait(&mut self, waiting: Instant, now: Instant) -> Shown {
        let limit = if self.was_live {
            RECOVER_TIMEOUT
        } else {
            APPEAR_TIMEOUT
        };
        if now.duration_since(waiting) < limit {
            return Shown::Frames;
        }
        let fault = match self.pending.take() {
            Some(Fault::OtherShape) => Fault::OtherShape,
            Some(fault) if self.was_live => fault,
            _ => Fault::NoWindow,
        };
        self.fail(fault)
    }

    fn unavailable(&mut self, error: PreviewError, waiting: Instant, now: Instant) -> Shown {
        match error {
            // The render surface is created a moment after the window, and a
            // rotation replaces it.
            PreviewError::NoSource
            | PreviewError::SourceUnavailable
            | PreviewError::InvalidCrop
            | PreviewError::EmptySlot
            | PreviewError::InvalidDestination => {
                self.pending = Some(Fault::Lost(detail(&error)));
                self.wait(waiting, now)
            }
            PreviewError::Win32(..) | PreviewError::Dwm(..) => {
                self.fail(Fault::System(error.to_string()))
            }
            other => self.fail(Fault::Lost(detail(&other))),
        }
    }

    /// Asks the engine for a render surface of the size shown, so that the
    /// compositor does not scale the picture.
    fn match_surface(&mut self, slot: PreviewRect, now: Instant) {
        let Some(source) = self
            .preview
            .as_ref()
            .and_then(ThumbnailPreview::source_size)
        else {
            return;
        };
        let wanted = (slot.right - slot.left, slot.bottom - slot.top);
        if self.trace != Some((slot, source)) {
            self.trace = Some((slot, source));
            if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                println!(
                    "engine_window panel={}x{} at={},{} source={}x{}",
                    wanted.0, wanted.1, slot.left, slot.top, source.0, source.1
                );
            }
        }
        if (source.0 - wanted.0).abs() <= 2 && (source.1 - wanted.1).abs() <= 2 {
            self.resize = None;
            return;
        }
        let since = match self.resize {
            Some((previous, since)) if previous == slot => since,
            _ => {
                self.resize = Some((slot, now));
                now
            }
        };
        if now.duration_since(since) < RESIZE_SETTLE {
            return;
        }
        // The engine rounds the size its own way: the same size is asked a
        // few times at most.
        let attempts = match self.resize_asked {
            Some((asked, attempts)) if asked == wanted => attempts,
            _ => 0,
        };
        if attempts < 3 {
            self.resize_asked = Some((wanted, attempts + 1));
            self.resize = Some((slot, now + Duration::from_secs(2)));
            *self.keeper.wanted.lock().unwrap() = Some(wanted);
        }
    }
}

impl Drop for EngineWindow {
    fn drop(&mut self) {
        self.keeper.stop.store(true, Ordering::Release);
        self.preview = None;
        self.backdrop = None;
        self.hook = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_path_is_described_in_every_language() {
        let paths = [
            PanelPath::EngineWindow,
            PanelPath::Frames(Fault::NoWindow),
            PanelPath::Frames(Fault::Lost(
                "The engine window is minimised, hidden or cloaked".into(),
            )),
            PanelPath::Frames(Fault::System(
                "DwmRegisterThumbnail: HRESULT 0x80263001".into(),
            )),
            PanelPath::Frames(Fault::OtherShape),
        ];
        for locale in [Locale::PtBr, Locale::EnUs, Locale::EsEs] {
            let texts: Vec<_> = paths.iter().map(|path| path.describe(locale)).collect();
            for (index, text) in texts.iter().enumerate() {
                assert!(text.ends_with('.') || text.ends_with('”'), "{text}");
                assert!(
                    texts[index + 1..].iter().all(|other| other != text),
                    "{text}"
                );
            }
            // The system's and the thumbnail code's own words are quoted verbatim.
            assert!(texts[2].ends_with("“The engine window is minimised, hidden or cloaked”"));
            assert!(texts[3].ends_with("“DwmRegisterThumbnail: HRESULT 0x80263001”"));
            assert!(texts[1].contains("30"));
        }
        assert_eq!(
            paths[0].describe(Locale::PtBr),
            "Painel: a janela do próprio motor, mostrada pela composição do Windows; nenhum quadro é copiado para o cliente."
        );
        assert_eq!(
            paths[1].describe(Locale::EnUs),
            "Panel: frames copied from the engine; the engine's window was not used because it did not appear with a render surface within 30 seconds."
        );
        assert_eq!(
            paths[3].describe(Locale::EsEs),
            "Panel: fotogramas copiados del motor; no se usó la ventana del motor porque Windows rechazó una llamada; mensaje del sistema: “DwmRegisterThumbnail: HRESULT 0x80263001”"
        );
    }

    #[test]
    fn a_window_counts_as_on_the_desktop_only_where_a_monitor_can_be() {
        let far = RECT {
            left: OFF_DESKTOP.0,
            top: OFF_DESKTOP.1,
            right: OFF_DESKTOP.0 + 1920,
            bottom: OFF_DESKTOP.1 + 1080,
        };
        assert!(!on_desktop(far));
        let (left, top) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            )
        };
        // Headless test sessions may report an empty desktop.
        if unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } > 100 {
            assert!(on_desktop(RECT {
                left: left + 10,
                top: top + 10,
                right: left + 90,
                bottom: top + 90,
            }));
        }
    }

    #[test]
    fn the_client_window_stays_one_pixel_short_of_covering_a_monitor() {
        let rect = |left, top, right, bottom| RECT {
            left,
            top,
            right,
            bottom,
        };
        let monitor = Some(rect(0, 1080, 1920, 2160));
        // Fullscreen, or larger than the monitor: the bottom edge moves up.
        assert_eq!(
            short_of_full_cover(rect(0, 1080, 1920, 2160), monitor),
            Some(2159)
        );
        assert_eq!(
            short_of_full_cover(rect(-8, 1072, 1928, 2168), monitor),
            Some(2159)
        );
        // Maximised above a taskbar, windowed, or one pixel short already.
        for kept in [
            rect(-8, 1072, 1928, 2120),
            rect(40, 1120, 1240, 1920),
            rect(0, 1080, 1920, 2159),
        ] {
            assert_eq!(short_of_full_cover(kept, monitor), None);
        }
        assert_eq!(short_of_full_cover(rect(0, 1080, 1920, 2160), None), None);
    }

    #[test]
    fn only_an_engine_started_by_this_launcher_is_looked_for() {
        // This test process started no emulator.
        assert_eq!(engine_process(std::process::id()), None);
    }

    #[test]
    fn a_session_without_an_engine_window_falls_back_and_says_so_once() {
        let mut panel = EngineWindow::watch(std::process::id());
        let slot = PreviewRect::from_points([0., 0.], [160., 90.], 1.);
        let start = Instant::now();
        assert_eq!(panel.show(slot, start), Shown::Frames);
        assert_eq!(panel.take_path_change(), None);
        assert_eq!(
            panel.show(slot, start + APPEAR_TIMEOUT - Duration::from_secs(1)),
            Shown::Frames
        );
        assert_eq!(panel.take_path_change(), None);
        assert_eq!(panel.show(slot, start + APPEAR_TIMEOUT), Shown::Frames);
        assert_eq!(
            panel.take_path_change(),
            Some(PanelPath::Frames(Fault::NoWindow))
        );
        assert_eq!(panel.show(slot, start + APPEAR_TIMEOUT * 2), Shown::Frames);
        assert_eq!(panel.take_path_change(), None);
    }
}
