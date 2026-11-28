//! Windows: the engine presents straight into the client's window.
//!
//! The engine runs with its real Qt window kept off the desktop, out of the
//! taskbar and away from the keyboard. Its render surface (the `subWin` child
//! that gfxstream's swapchain presents to) is taken out of that window and made
//! a child of a window of the client that covers the panel, so no frame passes
//! through this process. What the client draws over the panel is shown above
//! the engine's picture by a click-through overlay window (see `overlay`).
//!
//! The stock `-qt-hide-window` creates no render surface at all, and only the
//! engine's own process may cloak its window, so with stock flags the window
//! is moved off the desktop. The engine is asked to open it there and not to
//! bring it back (see `android::prepare_engine_window`); the thread here keeps
//! it there should it show up anyway.
//!
//! The panel falls back to copied frames, with the reason in the boot details,
//! when the surface does not appear, stops being usable, its picture on the
//! screen stands still while the engine's frames change, or the engine's
//! window stops answering.

mod child;
mod liveness;
mod overlay;

pub(crate) use liveness::SAMPLE;
pub(crate) use overlay::{FontCopy, remember_texture};

use crate::bluestacks::player_process::process_creation_time;
use crate::bluestacks::preview::PreviewRect;
use crate::ui::i18n::Locale;
use eframe::egui::{ColorImage, epaint::ClippedPrimitive};
use std::{
    cell::Cell,
    os::windows::io::{FromRawHandle, OwnedHandle},
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{ClientToScreen, DeleteObject},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{
            AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
        WindowsAndMessaging::{
            CallWindowProcW, DefWindowProcW, EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE,
            GWLP_WNDPROC, GetClassNameW, GetClientRect, GetForegroundWindow, GetSystemMetrics,
            GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
            IsWindow, IsWindowVisible, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT, SW_HIDE, SW_SHOWNOACTIVATE,
            SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            SendMessageTimeoutW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
            ShowWindowAsync, WM_NULL, WM_WINDOWPOSCHANGED, WNDPROC, WS_EX_APPWINDOW,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    },
};

/// Where the engine's windows are kept: far outside any monitor arrangement
/// and inside the 16-bit range of window coordinates.
pub(crate) const OFF_DESKTOP: (i32, i32) = (-25_000, -25_000);
/// The engine's render surface must show up within this time once the panel
/// has a picture from the frame stream.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(30);
/// A surface that was shown and went away (rotation recreates it) gets this
/// long to come back.
const RECOVER_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_millis(200);
const RESIZE_SETTLE: Duration = Duration::from_millis(250);
/// The engine's window is pinged this often, and waited for this long.
const PING_INTERVAL: Duration = Duration::from_millis(250);
/// An engine window that has not answered for this long is taken as hung:
/// the panel gives its surface back without waiting for it.
const HUNG_AFTER: Duration = Duration::from_secs(3);
const ENGINE_EXE: &str = "qemu-system-x86_64.exe";
/// A surface still in another shape than the panel this long after the panel
/// learnt the geometry asks for it again.
const TURN_RECHECK: Duration = Duration::from_secs(4);

/// Whether a surface of `size` has another shape than `slot`.
fn turned(slot: PreviewRect, size: (i32, i32)) -> bool {
    let shape = |width: i32, height: i32| width.max(1) as f32 / height.max(1) as f32;
    let (width, height) = (slot.right - slot.left, slot.bottom - slot.top);
    (shape(size.0, size.1) / shape(width, height) - 1.0).abs() > 0.03
}

/// Why the panel shows copied frames although the engine's surface was wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// No engine window with a render surface appeared in time.
    NoWindow,
    /// The surface was shown and then stopped being usable; why, in English.
    Lost(String),
    /// Windows refused a call: its name and code, verbatim.
    System(String),
    /// The picture on the screen stood still while the engine's frames changed.
    Stale,
    /// The engine's window stopped answering messages.
    Hung,
}

/// What the panel shows, for the boot details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelPath {
    /// The engine's render surface, presenting inside the client's window.
    EngineChild,
    /// Frames copied from the engine, and why.
    Frames(Fault),
}

impl PanelPath {
    pub(crate) fn describe(&self, locale: Locale) -> String {
        let fault = match self {
            Self::EngineChild => {
                return locale
                    .pick(
                        "Painel: a superfície de imagem do motor, dentro da janela do cliente; o motor apresenta direto na tela e nenhum quadro é copiado.",
                        "Panel: the engine's render surface, inside the client's window; the engine presents straight to the screen and no frame is copied.",
                        "Panel: la superficie de imagen del motor, dentro de la ventana del cliente; el motor presenta directo en la pantalla y no se copia ningún fotograma.",
                    )
                    .into();
            }
            Self::Frames(fault) => fault,
        };
        let lead = locale.pick(
            "Painel: quadros copiados do motor; a superfície do motor não foi usada porque",
            "Panel: frames copied from the engine; the engine's surface was not used because",
            "Panel: fotogramas copiados del motor; no se usó la superficie del motor porque",
        );
        let seconds = APPEAR_TIMEOUT.as_secs();
        let reason = match fault {
            Fault::NoWindow => match locale {
                Locale::PtBr => {
                    format!("ela não apareceu em {seconds} segundos.")
                }
                Locale::EnUs => {
                    format!("it did not appear within {seconds} seconds.")
                }
                Locale::EsEs => {
                    format!("no apareció en {seconds} segundos.")
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
            Fault::Stale => locale
                .pick(
                    "a imagem dela na tela parou enquanto a do Android mudava.",
                    "its picture on the screen stood still while Android's kept changing.",
                    "su imagen en la pantalla se quedó quieta mientras la de Android cambiaba.",
                )
                .to_string(),
            Fault::Hung => match locale {
                Locale::PtBr => format!(
                    "a janela do motor parou de responder por {} segundos.",
                    HUNG_AFTER.as_secs()
                ),
                Locale::EnUs => format!(
                    "the engine's window stopped responding for {} seconds.",
                    HUNG_AFTER.as_secs()
                ),
                Locale::EsEs => format!(
                    "la ventana del motor dejó de responder durante {} segundos.",
                    HUNG_AFTER.as_secs()
                ),
            },
        };
        format!("{lead} {reason}")
    }
}

const NO_SURFACE: &str = "The engine window has no visible render surface";

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

/// `rect` (physical client pixels of `host`) in screen pixels.
fn on_screen(host: HWND, rect: PreviewRect) -> Option<RECT> {
    let mut origin = POINT { x: 0, y: 0 };
    (unsafe { ClientToScreen(host, &mut origin) } != 0).then_some(RECT {
        left: origin.x + rect.left,
        top: origin.y + rect.top,
        right: origin.x + rect.right,
        bottom: origin.y + rect.bottom,
    })
}

/// Nothing covers the client's window at the middle and the corners of
/// `rect` (screen pixels).
fn sees(host: HWND, rect: RECT) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GA_ROOTOWNER, GetAncestor, WindowFromPoint,
    };
    if host.is_null() || unsafe { IsIconic(host) } != 0 {
        return false;
    }
    let inset = |a: i32, b: i32| (a + 2).min(b - 1);
    let points = [
        ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2),
        (inset(rect.left, rect.right), inset(rect.top, rect.bottom)),
        (rect.right - 3, rect.bottom - 3),
        (inset(rect.left, rect.right), rect.bottom - 3),
        (rect.right - 3, inset(rect.top, rect.bottom)),
    ];
    points.into_iter().all(|(x, y)| {
        let under = unsafe { WindowFromPoint(POINT { x, y }) };
        // The overlay is owned by the client's window and takes no input.
        !under.is_null()
            && (unsafe { GetAncestor(under, GA_ROOT) } == host
                || unsafe { GetAncestor(under, GA_ROOTOWNER) } == host)
    })
}

/// The screen inside `rect` at the liveness sample size, as RGB.
fn screen_sample(rect: RECT) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, GetDC, GetDIBits, HALFTONE, ReleaseDC, SRCCOPY, SelectObject,
        SetStretchBltMode, StretchBlt,
    };
    let (width, height) = (SAMPLE.0 as i32, SAMPLE.1 as i32);
    if rect.right - rect.left < width || rect.bottom - rect.top < height {
        return None;
    }
    unsafe {
        let screen = GetDC(ptr::null_mut());
        if screen.is_null() {
            return None;
        }
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap);
        SetStretchBltMode(memory, HALFTONE);
        let copied = StretchBlt(
            memory,
            0,
            0,
            width,
            height,
            screen,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SRCCOPY,
        ) != 0;
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let rows = if copied {
            GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                pixels.as_mut_ptr().cast(),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(ptr::null_mut(), screen);
        (rows == height).then(|| {
            pixels
                .chunks_exact(4)
                .flat_map(|bgra| [bgra[2], bgra[1], bgra[0]])
                .collect()
        })
    }
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
    /// The engine's main window.
    main: isize,
    /// Its render surface while that is still inside it.
    surface: Option<isize>,
}

/// Shared with the threads that keep the engine's windows off the desktop
/// and watch whether they answer.
struct Keeper {
    launcher: u32,
    stop: AtomicBool,
    /// The client's own window, to give the keyboard back to.
    host: AtomicIsize,
    found: Mutex<Option<Found>>,
    /// Size wanted for the render surface, taken by the thread.
    wanted: Mutex<Option<(i32, i32)>>,
    /// The render surface the client's window holds (set by the UI thread).
    adopted: AtomicIsize,
    epoch: Instant,
    /// When the engine's main window last answered a message (milliseconds
    /// after `epoch`, plus one; zero: never).
    answered: AtomicU64,
}

impl Keeper {
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64 + 1
    }

    /// Whether the engine's window answered within `within`; `None` before
    /// its first answer.
    fn answering(&self, within: Duration) -> Option<bool> {
        let answered = self.answered.load(Ordering::Acquire);
        (answered != 0).then(|| self.now().saturating_sub(answered) <= within.as_millis() as u64)
    }

    /// Pings the engine's main window from a thread of its own: no other code
    /// of the client ever waits for that window without a time limit.
    fn watch(&self) {
        while !self.stop.load(Ordering::Acquire) {
            let main = self.found.lock().unwrap().map_or(0, |found| found.main);
            if main != 0 {
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
                    self.answered.store(self.now(), Ordering::Release);
                }
            }
            thread::sleep(PING_INTERVAL);
        }
    }

    fn run(&self) {
        // Window coordinates of another process are physical pixels.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let mut engine: Option<(u32, u64)> = None;
        let mut first_seen: Option<Instant> = None;
        let mut restyled = 0;
        let mut main_window: isize = 0;
        // The windows asked to move lately: the engine's thread may take a while.
        let mut moved: Vec<(isize, Instant)> = Vec::new();
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
                if !windows.contains(&main_window) {
                    main_window = 0;
                }
                let mut inside = None;
                for window in windows.iter().map(|window| *window as HWND) {
                    moved.retain(|(_, at)| at.elapsed() < Duration::from_millis(100));
                    if window_rect(window).is_some_and(on_desktop)
                        && !moved.iter().any(|(which, _)| *which == window as isize)
                    {
                        // Asynchronous, as everything done to the engine's
                        // windows here: its thread may be busy or hung.
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
                    let surface = render_surface(window);
                    // The surface leaves the main window: the window it was
                    // found in stays the main one.
                    let main = unsafe { GetWindow(window, GW_OWNER) }.is_null()
                        && (surface.is_some() || window as isize == main_window);
                    if main {
                        main_window = window as isize;
                        inside = surface;
                        // An engine that puts its styles back keeps its button.
                        if restyled < 3 && self.keep_out_of_the_taskbar(window) {
                            restyled += 1;
                        }
                        self.resize(window, surface);
                        hidden = window_rect(window).is_some_and(|rect| !on_desktop(rect));
                    }
                }
                self.give_the_keyboard_back(pid);
                *self.found.lock().unwrap() = Some(Found {
                    pid,
                    created,
                    hidden,
                    main: main_window,
                    surface: inside,
                });
            }
            // The first seconds of the engine's window are watched closely.
            let settled =
                hidden && first_seen.is_some_and(|seen| seen.elapsed() > Duration::from_secs(5));
            thread::sleep(if settled {
                CHECK_INTERVAL
            } else {
                Duration::from_millis(5)
            });
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

thread_local! {
    /// The window procedure of the client's window before `host_proc`.
    static HOST_PROC: Cell<isize> = const { Cell::new(0) };
}

fn host_proc_address() -> isize {
    host_proc as *const () as isize
}

/// Keeps the overlay over the panel while Windows moves the client's window,
/// between two frames of the client.
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
    if message == WM_WINDOWPOSCHANGED {
        overlay::follow(window);
    }
    result
}

/// The client's window procedure is `host_proc` while the panel holds the
/// engine's surface.
struct HostHook {
    host: isize,
}

impl HostHook {
    fn install(host: HWND) -> Self {
        let previous = unsafe { SetWindowLongPtrW(host, GWLP_WNDPROC, host_proc_address()) };
        HOST_PROC.with(|cell| cell.set(previous));
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

/// What the panel may do in this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The engine's render surface covers `0` (physical client pixels): what
    /// the client draws there is shown by the overlay.
    Child(PreviewRect),
    /// Not yet, or not at the moment: paint the copied frames.
    Frames,
    /// The client's window is minimised: nothing is seen and nothing is needed.
    Unseen,
}

pub(crate) struct EngineWindow {
    liveness: liveness::Liveness,
    could_sample: bool,
    /// The share of the picture the client drew over in the last frame.
    overlay_share: f32,
    /// Where the engine's picture was put last (physical client pixels).
    shown_at: Option<PreviewRect>,
    keeper: Arc<Keeper>,
    threads: Vec<thread::JoinHandle<()>>,
    /// The client's window that holds the render surface.
    holder: Option<child::Holder>,
    overlay: Option<overlay::Overlay>,
    hook: Option<HostHook>,
    /// Since when the panel has waited for a usable surface.
    waiting: Option<Instant>,
    /// What kept the surface from being shown, should it last.
    pending: Option<Fault>,
    was_live: bool,
    fault: Option<Fault>,
    /// The slot the render surface should match, since when, and what was asked.
    resize: Option<(PreviewRect, Instant)>,
    resize_asked: Option<((i32, i32), u8)>,
    /// When the surface was last found in another shape than the panel
    /// (Android turned), and whether the panel should learn the new geometry.
    turned_at: Option<Instant>,
    geometry_wanted: bool,
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
            adopted: AtomicIsize::new(0),
            epoch: Instant::now(),
            answered: AtomicU64::new(0),
        });
        let mut threads = Vec::new();
        let worker = keeper.clone();
        threads.extend(
            thread::Builder::new()
                .name("engine window".into())
                .spawn(move || worker.run())
                .ok(),
        );
        let watcher = keeper.clone();
        threads.extend(
            thread::Builder::new()
                .name("engine window watchdog".into())
                .spawn(move || watcher.watch())
                .ok(),
        );
        Self {
            liveness: liveness::Liveness::default(),
            could_sample: false,
            overlay_share: 0.0,
            shown_at: None,
            keeper,
            threads,
            holder: None,
            overlay: None,
            hook: None,
            waiting: None,
            pending: None,
            was_live: false,
            fault: None,
            resize: None,
            resize_asked: None,
            turned_at: None,
            geometry_wanted: false,
            reported: None,
            report_pending: None,
            trace: None,
        }
    }

    /// The client's own window, as soon as it is known.
    pub(crate) fn set_host(&mut self, host: isize) {
        self.keeper.host.store(host, Ordering::Release);
    }

    /// Content check of the picture last placed: `true` (and the panel
    /// returns to copied frames for the session) when it is stale.
    pub(crate) fn check_live(
        &mut self,
        now: Instant,
        sample: impl FnOnce() -> Option<liveness::EngineSample>,
    ) -> bool {
        let host = self.keeper.host.load(Ordering::Acquire) as HWND;
        let place = self.shown_at.and_then(|at| on_screen(host, at));
        // Small static overlays (control hints) do not hide the picture; a
        // dialog over most of it does.
        let hidden = self.overlay_share > 0.25;
        let can_sample = !hidden && place.is_some_and(|rect| sees(host, rect));
        if can_sample != self.could_sample {
            self.could_sample = can_sample;
            if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                println!("engine_liveness sampling={can_sample}");
            }
        }
        let judged = self.liveness.judged;
        let stale = self
            .liveness
            .poll(now, can_sample, sample, || place.and_then(screen_sample));
        if self.liveness.judged != judged
            && self.liveness.judged % 20 == 1
            && std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some()
        {
            println!(
                "engine_liveness judged={} followed={} overlay_redrawn={}",
                self.liveness.judged,
                self.liveness.followed,
                self.overlay.as_ref().map_or(0, |overlay| overlay.redrawn),
            );
        }
        if stale {
            if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                println!(
                    "engine_liveness stale judged={} followed={}",
                    self.liveness.judged, self.liveness.followed
                );
            }
            self.fail(Fault::Stale);
        }
        stale
    }

    /// A liveness sample is on its way.
    pub(crate) fn sampling(&self) -> bool {
        self.liveness.waiting()
    }

    /// Still trying to show the engine's surface (it has not been given up).
    pub(crate) fn active(&self) -> bool {
        self.fault.is_none()
    }

    /// Whether Android turned since the panel was laid out: the copied
    /// frames tell the new geometry. Once per turn.
    pub(crate) fn take_geometry_request(&mut self) -> bool {
        std::mem::take(&mut self.geometry_wanted)
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
        self.overlay = None;
        self.drop_holder();
        self.hook = None;
        self.report(PanelPath::Frames(fault.clone()));
        self.fault = Some(fault);
        Shown::Frames
    }

    /// Stops showing the engine's surface for now (the panel is not drawn).
    pub(crate) fn hide(&mut self) {
        if let Some(holder) = &mut self.holder {
            holder.hide();
        }
        if let Some(overlay) = &mut self.overlay {
            overlay.hide();
        }
    }

    /// Whether the engine's window answers (or has not been asked yet).
    fn engine_answers(&self) -> bool {
        self.keeper
            .answering(Duration::from_millis(1500))
            .unwrap_or(true)
    }

    fn drop_holder(&mut self) {
        if let Some(holder) = self.holder.take() {
            holder.release(self.engine_answers());
        }
        self.keeper.adopted.store(0, Ordering::Release);
    }

    /// What the client drew over the panel at `slot` in this frame:
    /// `covered`, the parts it covers (physical client pixels), and
    /// `primitives`, the same shapes tessellated at `scale` pixels per point.
    /// They are shown on top of the engine's picture; `font` gives the font
    /// atlas when the overlay must be redrawn.
    pub(crate) fn draw_over(
        &mut self,
        slot: PreviewRect,
        covered: &[PreviewRect],
        primitives: &[ClippedPrimitive],
        scale: f32,
        font: impl FnOnce() -> Arc<ColorImage>,
    ) {
        self.overlay_share = child::share(slot, &child::cut_out(slot, covered));
        let host = self.keeper.host.load(Ordering::Acquire) as HWND;
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if self.holder.as_mut().and_then(child::Holder::held).is_none() {
            overlay.hide();
            return;
        }
        if let Err(fault) = overlay.show(host, slot, primitives, scale, font) {
            self.fail(fault);
        }
    }

    /// Whether the overlay shows anything at the moment.
    #[cfg(test)]
    fn overlay_visible(&self) -> bool {
        self.overlay.as_ref().is_some_and(overlay::Overlay::visible)
    }

    /// Places the engine's surface over `slot` (the fitted Android picture
    /// in physical client pixels of the client's window), or says why not.
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
        if self.holder.is_some() && self.keeper.answering(HUNG_AFTER) == Some(false) {
            // The joined input queues must not let the engine's thread hold
            // the client's input any longer.
            if let Some(holder) = &mut self.holder {
                holder.detach();
            }
            return self.fail(Fault::Hung);
        }
        self.show_child(host as HWND, slot, found, waiting, now)
    }

    /// The engine's render surface, held by a window of the client at `slot`.
    fn show_child(
        &mut self,
        host: HWND,
        slot: PreviewRect,
        found: Found,
        waiting: Instant,
        now: Instant,
    ) -> Shown {
        if self.holder.is_none() {
            match child::Holder::create(host) {
                Ok(holder) => self.holder = Some(holder),
                Err(fault) => return self.fail(fault),
            }
        }
        if self.overlay.is_none() {
            match overlay::Overlay::create(host) {
                Ok(overlay) => self.overlay = Some(overlay),
                Err(fault) => return self.fail(fault),
            }
            self.hook = Some(HostHook::install(host));
        }
        let answers = self.engine_answers();
        let holder = self.holder.as_mut().unwrap();
        let mut surface = holder.held();
        if surface.is_none()
            && let Some(inside) = found.surface
            && answers
        {
            // Placed first, so the surface lands where it is shown.
            holder.place(slot);
            if let Err(fault) = holder.adopt(inside, found.main) {
                return self.fail(fault);
            }
            surface = Some(inside);
        }
        self.keeper
            .adopted
            .store(surface.unwrap_or(0), Ordering::Release);
        let Some(surface) = surface else {
            // A rotation replaces the surface inside the engine's window.
            holder.hide();
            self.pending = Some(Fault::Lost(NO_SURFACE.into()));
            return self.wait(waiting, now);
        };
        holder.place(slot);
        let Some(size) = holder.centre(surface) else {
            holder.hide();
            self.pending = Some(Fault::Lost(NO_SURFACE.into()));
            return self.wait(waiting, now);
        };
        self.waiting = Some(now);
        self.was_live = true;
        self.pending = None;
        // The engine keeps the shape of the Android screen: a surface of
        // another shape than the panel means Android turned.
        if turned(slot, size)
            && self
                .turned_at
                .is_none_or(|at| now.duration_since(at) > TURN_RECHECK)
        {
            self.turned_at = Some(now);
            self.geometry_wanted = true;
        }
        self.match_size(slot, size, now);
        self.report(PanelPath::EngineChild);
        if self.shown_at != Some(slot) {
            self.liveness.reset();
            self.shown_at = Some(slot);
        }
        Shown::Child(slot)
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
            Some(fault) if self.was_live => fault,
            _ => Fault::NoWindow,
        };
        self.fail(fault)
    }

    /// Asks the engine for a render surface of the size shown, so that the
    /// picture is not scaled.
    fn match_size(&mut self, slot: PreviewRect, source: (i32, i32), now: Instant) {
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
        self.overlay = None;
        self.drop_holder();
        self.hook = None;
        for thread in self.threads.drain(..) {
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
            PanelPath::EngineChild,
            PanelPath::Frames(Fault::NoWindow),
            PanelPath::Frames(Fault::Lost(NO_SURFACE.into())),
            PanelPath::Frames(Fault::System(
                "SetParent: Access is denied. (os error 5)".into(),
            )),
            PanelPath::Frames(Fault::Stale),
            PanelPath::Frames(Fault::Hung),
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
            // English details and the system's words are quoted verbatim.
            assert!(texts[2].ends_with(&format!("“{NO_SURFACE}”")));
            assert!(texts[3].ends_with("“SetParent: Access is denied. (os error 5)”"));
            assert!(texts[1].contains("30"));
            assert!(texts[5].contains('3'));
        }
        assert_eq!(
            paths[0].describe(Locale::EnUs),
            "Panel: the engine's render surface, inside the client's window; the engine presents straight to the screen and no frame is copied."
        );
        assert_eq!(
            paths[1].describe(Locale::EnUs),
            "Panel: frames copied from the engine; the engine's surface was not used because it did not appear within 30 seconds."
        );
        assert_eq!(
            paths[5].describe(Locale::PtBr),
            "Painel: quadros copiados do motor; a superfície do motor não foi usada porque a janela do motor parou de responder por 3 segundos."
        );
        assert_eq!(
            paths[3].describe(Locale::EsEs),
            "Panel: fotogramas copiados del motor; no se usó la superficie del motor porque Windows rechazó una llamada; mensaje del sistema: “SetParent: Access is denied. (os error 5)”"
        );
    }

    #[test]
    fn a_surface_of_another_shape_means_android_turned() {
        let slot = PreviewRect::from_points([0., 0.], [1154., 648.], 1.).unwrap();
        // Same shape, rounded by the engine, smaller or larger.
        for size in [(1152, 648), (1600, 900), (371, 209)] {
            assert!(!turned(slot, size), "{size:?}");
        }
        for size in [(364, 648), (648, 1152), (1000, 1000)] {
            assert!(turned(slot, size), "{size:?}");
        }
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
        assert!(!panel.overlay_visible());
    }

    #[test]
    fn an_engine_window_is_judged_by_its_last_answer() {
        let keeper = Keeper {
            launcher: 0,
            stop: AtomicBool::new(false),
            host: AtomicIsize::new(0),
            found: Mutex::new(None),
            wanted: Mutex::new(None),
            adopted: AtomicIsize::new(0),
            epoch: Instant::now() - Duration::from_secs(60),
            answered: AtomicU64::new(0),
        };
        assert_eq!(keeper.answering(HUNG_AFTER), None);
        keeper.answered.store(keeper.now(), Ordering::Release);
        assert_eq!(keeper.answering(HUNG_AFTER), Some(true));
        keeper.answered.store(
            keeper.now() - HUNG_AFTER.as_millis() as u64 - 100,
            Ordering::Release,
        );
        assert_eq!(keeper.answering(HUNG_AFTER), Some(false));
        assert_eq!(keeper.answering(HUNG_AFTER * 2), Some(true));
    }
}
