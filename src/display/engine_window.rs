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
//! bring it back (see `android::prepare_engine_window`); the keeper thread
//! keeps it there should it show up anyway, and brings the engine's dialogs
//! into view.
//!
//! The panel copies frames instead, with the reason in the boot details, when
//! the surface does not appear, stops being usable, its picture on the screen
//! stands still while the engine's frames change or stays one flat colour
//! while the engine's frame has a picture (both tried again later), or the
//! engine's window stops answering. The client's UI thread never waits for
//! the engine's thread (see `child` and `keeper`).

mod child;
mod host;
mod keeper;
mod liveness;
mod overlay;

pub(crate) use liveness::SAMPLE;
pub(crate) use overlay::{FontCopy, remember_texture};

use crate::bluestacks::player_process::process_creation_time;
use crate::bluestacks::preview::PreviewRect;
use crate::ui::i18n::Locale;
use eframe::egui::{ColorImage, epaint::ClippedPrimitive};
use keeper::{HUNG_AFTER, Keeper};
use std::{
    os::windows::io::{FromRawHandle, OwnedHandle},
    ptr,
    sync::{Arc, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, POINT, RECT},
    Graphics::Gdi::{ClientToScreen, DeleteObject},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
    UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetSystemMetrics, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
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
/// After the content check gave the surface up, the panel tries it again
/// this much later, at most this many times a session.
const RETRY_AFTER: Duration = Duration::from_secs(60);
const RETRIES: u8 = 3;
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

/// Why a surface that was shown stopped being usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lost {
    /// The engine's window has no visible render surface (any more).
    NoSurface,
    /// The render surface belongs to another thread than the engine's window.
    OtherThread,
}

/// Why the panel shows copied frames although the engine's surface was wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// No engine window with a render surface appeared in time.
    NoWindow,
    /// The surface was shown and then stopped being usable.
    Lost(Lost),
    /// Windows refused a call: its name and code, verbatim.
    System(String),
    /// The picture on the screen stood still while the engine's frames
    /// changed: tried again later.
    Stale,
    /// The panel's area of the screen stayed one flat colour while the
    /// engine's frame had a picture: tried again later.
    Blank,
    /// The engine's window stopped answering messages.
    Hung,
}

impl Fault {
    /// Faults found by the content check: the surface is tried again later.
    fn retried(&self) -> bool {
        matches!(self, Self::Stale | Self::Blank)
    }
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
                Locale::PtBr => format!("ela não apareceu em {seconds} segundos."),
                Locale::EnUs => format!("it did not appear within {seconds} seconds."),
                Locale::EsEs => format!("no apareció en {seconds} segundos."),
            },
            Fault::Lost(Lost::NoSurface) => locale
                .pick(
                    "a janela do motor ficou sem uma superfície de imagem visível.",
                    "the engine's window no longer had a visible render surface.",
                    "la ventana del motor se quedó sin una superficie de imagen visible.",
                )
                .to_string(),
            Fault::Lost(Lost::OtherThread) => locale
                .pick(
                    "a superfície de imagem não pertencia à mesma linha de execução da janela do motor.",
                    "the render surface did not belong to the same thread as the engine's window.",
                    "la superficie de imagen no pertenecía al mismo hilo que la ventana del motor.",
                )
                .to_string(),
            Fault::System(detail) => format!(
                "{} “{detail}”",
                locale.pick(
                    "o Windows recusou uma chamada; mensagem do sistema:",
                    "Windows refused a call; the system's message:",
                    "Windows rechazó una llamada; mensaje del sistema:",
                )
            ),
            Fault::Stale => match locale {
                Locale::PtBr => format!(
                    "a imagem dela na tela parou enquanto a do Android mudava. O painel tenta de novo em {} segundos.",
                    RETRY_AFTER.as_secs()
                ),
                Locale::EnUs => format!(
                    "its picture on the screen stood still while Android's kept changing. The panel tries again in {} seconds.",
                    RETRY_AFTER.as_secs()
                ),
                Locale::EsEs => format!(
                    "su imagen en la pantalla se quedó quieta mientras la de Android cambiaba. El panel lo vuelve a intentar en {} segundos.",
                    RETRY_AFTER.as_secs()
                ),
            },
            Fault::Blank => match locale {
                Locale::PtBr => format!(
                    "a área dela na tela ficou de uma cor só enquanto o Android mostrava uma imagem. O painel tenta de novo em {} segundos.",
                    RETRY_AFTER.as_secs()
                ),
                Locale::EnUs => format!(
                    "its area of the screen stayed one flat colour while Android showed a picture. The panel tries again in {} seconds.",
                    RETRY_AFTER.as_secs()
                ),
                Locale::EsEs => format!(
                    "su área de la pantalla se quedó de un solo color mientras Android mostraba una imagen. El panel lo vuelve a intentar en {} segundos.",
                    RETRY_AFTER.as_secs()
                ),
            },
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

/// The compositor keeps `window` out of sight (another virtual desktop, an
/// app being suspended).
fn cloaked(window: HWND) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    let mut cloaked = 0u32;
    let read = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED as u32,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    read == 0 && cloaked != 0
}

/// The client's window is shown, and the middle and the corners of `rect`
/// (screen pixels) are on a monitor with nothing covering the window there.
fn sees(host: HWND, rect: RECT) -> bool {
    use windows_sys::Win32::{
        Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint},
        UI::WindowsAndMessaging::{GA_ROOT, GA_ROOTOWNER, GetAncestor, WindowFromPoint},
    };
    if host.is_null()
        || unsafe { IsIconic(host) } != 0
        || unsafe { IsWindowVisible(host) } == 0
        || cloaked(host)
    {
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
        // A part of the panel off every monitor is not on the screen at all,
        // though the window is still found there.
        let point = POINT { x, y };
        if unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) }.is_null() {
            return false;
        }
        let under = unsafe { WindowFromPoint(point) };
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
    /// The client's window that holds the render surface.
    holder: Option<child::Holder>,
    overlay: Option<overlay::Overlay>,
    hook: Option<host::HostHook>,
    /// Since when the panel has waited for a usable surface.
    waiting: Option<Instant>,
    /// What kept the surface from being shown, should it last.
    pending: Option<Fault>,
    was_live: bool,
    fault: Option<Fault>,
    /// When a stale surface is tried again, and how often it was.
    retry_at: Option<Instant>,
    retries: u8,
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
        let keeper = Arc::new(Keeper::new(launcher));
        let worker = keeper.clone();
        let _ = thread::Builder::new()
            .name("engine window".into())
            .spawn(move || worker.run());
        let watcher = keeper.clone();
        let _ = thread::Builder::new()
            .name("engine window watchdog".into())
            .spawn(move || watcher.watch());
        Self {
            liveness: liveness::Liveness::default(),
            could_sample: false,
            overlay_share: 0.0,
            shown_at: None,
            keeper,
            holder: None,
            overlay: None,
            hook: None,
            waiting: None,
            pending: None,
            was_live: false,
            fault: None,
            retry_at: None,
            retries: 0,
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
    /// returns to copied frames for a while) when it is stale.
    pub(crate) fn check_live(
        &mut self,
        now: Instant,
        sample: impl FnOnce() -> Option<liveness::EngineSample>,
    ) -> bool {
        let host = self.keeper.host.load(Ordering::Acquire) as HWND;
        let place = self.shown_at.and_then(|at| on_screen(host, at));
        // Small static overlays (control hints) do not hide the picture; a
        // dialog over most of it does, and so do a locked session and a
        // display that is off.
        let hidden = self.overlay_share > 0.25 || host::screen_away();
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
        let stats = std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some();
        if self.liveness.judged != judged
            && stats
            && let Some(found @ (liveness::Evidence::StoodStill | liveness::Evidence::Blank)) =
                self.liveness.last_evidence
        {
            println!("engine_liveness evidence={found:?}");
        }
        if self.liveness.judged != judged && self.liveness.judged % 20 == 1 && stats {
            println!(
                "engine_liveness judged={} followed={} overlay_redrawn={}",
                self.liveness.judged,
                self.liveness.followed,
                self.overlay.as_ref().map_or(0, |overlay| overlay.redrawn),
            );
        }
        if stale {
            if stats {
                println!(
                    "engine_liveness stale judged={} followed={}",
                    self.liveness.judged, self.liveness.followed
                );
            }
            let fault = if self.liveness.last_evidence == Some(liveness::Evidence::Blank) {
                Fault::Blank
            } else {
                Fault::Stale
            };
            self.fail(fault, now);
        }
        stale
    }

    /// A liveness sample is on its way.
    pub(crate) fn sampling(&self) -> bool {
        self.liveness.waiting()
    }

    /// Still trying to show the engine's surface, or going to try it again.
    pub(crate) fn active(&self) -> bool {
        self.fault.is_none() || self.retry_at.is_some()
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

    fn fail(&mut self, fault: Fault, now: Instant) -> Shown {
        self.overlay = None;
        self.drop_holder();
        self.hook = None;
        // The engine's own picture is not wanted while frames are copied.
        self.keeper.quiet.store(true, Ordering::Release);
        self.retry_at = (fault.retried() && self.retries < RETRIES).then(|| now + RETRY_AFTER);
        self.report(PanelPath::Frames(fault.clone()));
        self.fault = Some(fault);
        Shown::Frames
    }

    /// The surface given up by the content check is tried again.
    fn retry(&mut self, now: Instant) {
        self.retries += 1;
        self.retry_at = None;
        self.fault = None;
        self.keeper.hung.store(false, Ordering::Release);
        self.keeper.mark_answered();
        self.keeper.quiet.store(false, Ordering::Release);
        self.waiting = Some(now);
        self.was_live = false;
        self.pending = None;
        self.shown_at = None;
        self.liveness.reset();
        if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
            println!("engine_child retry={}", self.retries);
        }
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

    fn drop_holder(&mut self) {
        self.keeper.adopted.store(0, Ordering::Release);
        if let Some(holder) = self.holder.take() {
            holder.release();
        }
    }

    /// What the client drew over the panel at `slot` in this frame:
    /// `covered`, the parts it covers (physical client pixels), and
    /// `primitives`, the same shapes tessellated at `scale` pixels per point.
    /// They are shown on top of the engine's picture; `font` gives the font
    /// atlas when the overlay must be redrawn. `true` when a change waits for
    /// its turn (the caller should paint again shortly).
    pub(crate) fn draw_over(
        &mut self,
        slot: PreviewRect,
        covered: &[PreviewRect],
        primitives: &[ClippedPrimitive],
        scale: f32,
        font: impl FnOnce() -> Arc<ColorImage>,
    ) -> bool {
        self.overlay_share = child::share(slot, &child::cut_out(slot, covered));
        let host = self.keeper.host.load(Ordering::Acquire) as HWND;
        let Some(overlay) = &mut self.overlay else {
            return false;
        };
        if self.keeper.adopted.load(Ordering::Acquire) == 0 {
            overlay.hide();
            return false;
        }
        match overlay.show(host, slot, primitives, scale, font) {
            Ok(waiting) => waiting,
            Err(fault) => {
                self.fail(fault, Instant::now());
                false
            }
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
            if self.retry_at.is_some_and(|at| now >= at) {
                self.retry(now);
            } else {
                return Shown::Frames;
            }
        }
        // The watchdog took the surface back from a hung engine (whether or
        // not the panel was drawn meanwhile).
        if self.keeper.hung.load(Ordering::Acquire) {
            return self.fail(Fault::Hung, now);
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
        self.show_child(host as HWND, slot, found, waiting, now)
    }

    /// The engine's render surface, held by a window of the client at `slot`.
    fn show_child(
        &mut self,
        host: HWND,
        slot: PreviewRect,
        found: keeper::Found,
        waiting: Instant,
        now: Instant,
    ) -> Shown {
        if self.holder.is_none() {
            match child::Holder::create(host) {
                Ok(holder) => self.holder = Some(holder),
                Err(fault) => return self.fail(fault, now),
            }
        }
        if self.overlay.is_none() {
            match overlay::Overlay::create(host) {
                Ok(overlay) => self.overlay = Some(overlay),
                Err(fault) => return self.fail(fault, now),
            }
            self.hook = Some(host::HostHook::install(host));
        }
        let answers = self
            .keeper
            .answering(Duration::from_millis(1500))
            .unwrap_or(true);
        let holder = self.holder.as_mut().unwrap();
        let surface = match holder.held() {
            Ok(surface) => surface,
            Err(fault) => return self.fail(fault, now),
        };
        if surface.is_none()
            && !holder.taking()
            && let Some(inside) = found.surface
            && answers
        {
            // Placed first, so the surface lands where it is shown.
            holder.place(slot);
            self.keeper
                .ui_thread
                .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
            if let Err(fault) = holder.adopt(inside, found.main) {
                return self.fail(fault, now);
            }
        }
        self.keeper
            .adopted
            .store(surface.unwrap_or(0), Ordering::Release);
        let Some(surface) = surface else {
            if !holder.taking() {
                // A rotation replaces the surface inside the engine's window.
                holder.hide();
            }
            self.pending = Some(Fault::Lost(Lost::NoSurface));
            return self.wait(waiting, now);
        };
        holder.place(slot);
        let Some(size) = holder.centre(surface) else {
            holder.hide();
            self.pending = Some(Fault::Lost(Lost::NoSurface));
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
        self.fail(fault, now)
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
    /// Nothing here waits for the engine: the surface is given back from a
    /// thread of its own and the keeper's threads end by themselves.
    fn drop(&mut self) {
        self.keeper.stop.store(true, Ordering::Release);
        self.overlay = None;
        self.drop_holder();
        self.hook = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyWindow, WS_OVERLAPPEDWINDOW};

    #[test]
    fn the_panel_path_is_described_in_every_language() {
        let paths = [
            PanelPath::EngineChild,
            PanelPath::Frames(Fault::NoWindow),
            PanelPath::Frames(Fault::Lost(Lost::NoSurface)),
            PanelPath::Frames(Fault::System(
                "SetParent: Access is denied. (os error 5)".into(),
            )),
            PanelPath::Frames(Fault::Stale),
            PanelPath::Frames(Fault::Hung),
            PanelPath::Frames(Fault::Lost(Lost::OtherThread)),
            PanelPath::Frames(Fault::Blank),
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
            // The system's words are quoted verbatim.
            assert!(texts[3].ends_with("“SetParent: Access is denied. (os error 5)”"));
            assert!(texts[1].contains("30"));
            assert!(texts[4].contains("60"));
            assert!(texts[5].contains('3'));
            assert!(texts[7].contains("60"));
        }
        assert_eq!(
            paths[0].describe(Locale::EnUs),
            "Panel: the engine's render surface, inside the client's window; the engine presents straight to the screen and no frame is copied."
        );
        assert_eq!(
            paths[2].describe(Locale::PtBr),
            "Painel: quadros copiados do motor; a superfície do motor não foi usada porque a janela do motor ficou sem uma superfície de imagem visível."
        );
        assert_eq!(
            paths[5].describe(Locale::PtBr),
            "Painel: quadros copiados do motor; a superfície do motor não foi usada porque a janela do motor parou de responder por 3 segundos."
        );
        assert_eq!(
            paths[6].describe(Locale::EsEs),
            "Panel: fotogramas copiados del motor; no se usó la superficie del motor porque la superficie de imagen no pertenecía al mismo hilo que la ventana del motor."
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
    fn this_test_process_started_no_engine_to_look_for() {
        // Only an engine whose parent chain reaches the launcher counts; this
        // process launched none (other engines on the machine are ignored).
        assert_eq!(engine_process(std::process::id()), None);
    }

    #[test]
    fn a_session_whose_engine_window_never_appears_falls_back_and_says_so_once() {
        let host = child::tests::window(WS_OVERLAPPEDWINDOW, ptr::null_mut());
        let mut panel = EngineWindow::watch(std::process::id());
        panel.set_host(host as isize);
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
        assert!(panel.keeper.quiet.load(Ordering::Acquire));
        drop(panel);
        unsafe { DestroyWindow(host) };
    }

    /// A surface given up by the content check comes back after a while,
    /// a limited number of times; meanwhile the engine's window is quiet.
    #[test]
    fn a_client_window_that_is_not_shown_is_not_sampled() {
        let host = child::tests::window(WS_OVERLAPPEDWINDOW, ptr::null_mut());
        let rect = RECT {
            left: 0,
            top: 0,
            right: 320,
            bottom: 180,
        };
        // Created without being shown: nothing of it is on the screen.
        assert!(!cloaked(host));
        assert!(!sees(host, rect));
        assert!(!sees(ptr::null_mut(), rect));
        assert!(!Fault::Hung.retried() && Fault::Stale.retried() && Fault::Blank.retried());
        unsafe { DestroyWindow(host) };
    }

    #[test]
    fn a_stale_surface_is_tried_again_later_a_few_times() {
        let host = child::tests::window(WS_OVERLAPPEDWINDOW, ptr::null_mut());
        let engine = child::tests::Engine::start();
        let mut panel = EngineWindow::watch(std::process::id());
        panel.set_host(host as isize);
        let found = keeper::Found {
            pid: std::process::id(),
            created: 0,
            hidden: true,
            main: engine.main,
            surface: Some(engine.surface),
        };
        *panel.keeper.found.lock().unwrap() = Some(found);
        let slot = PreviewRect::from_points([0., 0.], [320., 180.], 1.);
        let mut clock = Instant::now();
        for attempt in 0..=RETRIES {
            let mut shown = Shown::Frames;
            child::tests::pump_until(
                || {
                    shown = panel.show(slot, clock);
                    matches!(shown, Shown::Child(_))
                },
                Duration::from_secs(5),
            );
            assert!(
                matches!(shown, Shown::Child(_)),
                "attempt {attempt}: {shown:?}"
            );
            assert_eq!(panel.take_path_change(), Some(PanelPath::EngineChild));
            // Both findings of the content check are tried again.
            let fault = if attempt % 2 == 0 {
                Fault::Stale
            } else {
                Fault::Blank
            };
            assert_eq!(panel.fail(fault.clone(), clock), Shown::Frames);
            assert!(panel.keeper.quiet.load(Ordering::Acquire));
            assert_eq!(panel.take_path_change(), Some(PanelPath::Frames(fault)));
            // The surface goes home (the keeper would report it there again).
            child::tests::pump_until(
                || unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetParent(engine.surface as HWND)
                        == engine.main as HWND
                },
                Duration::from_secs(5),
            );
            assert_eq!(panel.show(slot, clock + RETRY_AFTER / 2), Shown::Frames);
            clock += RETRY_AFTER;
            if attempt < RETRIES {
                assert!(panel.active());
            }
        }
        // Retries used up: copied frames for the rest of the session.
        assert!(!panel.active());
        assert_eq!(panel.show(slot, clock + RETRY_AFTER * 5), Shown::Frames);
        drop(panel);
        child::tests::pump_until(|| false, Duration::from_millis(300));
        drop(engine);
        unsafe { DestroyWindow(host) };
    }

    /// The panel's whole path against windows of another thread: the surface
    /// is taken, shown, the engine hangs, the watchdog takes the surface back
    /// on its own (nothing drawn meanwhile), and the panel falls back.
    #[test]
    fn a_hung_engine_loses_its_surface_without_the_panel_waiting_for_it() {
        let host = child::tests::window(WS_OVERLAPPEDWINDOW, ptr::null_mut());
        let engine = child::tests::Engine::start();
        let mut panel = EngineWindow::watch(std::process::id());
        panel.set_host(host as isize);
        *panel.keeper.found.lock().unwrap() = Some(keeper::Found {
            pid: std::process::id(),
            created: 0,
            hidden: true,
            main: engine.main,
            surface: Some(engine.surface),
        });
        let slot = PreviewRect::from_points([0., 0.], [320., 180.], 1.);
        let mut shown = Shown::Frames;
        child::tests::pump_until(
            || {
                shown = panel.show(slot, Instant::now());
                matches!(shown, Shown::Child(_))
            },
            Duration::from_secs(5),
        );
        assert!(matches!(shown, Shown::Child(_)), "{shown:?}");
        assert_eq!(panel.take_path_change(), Some(PanelPath::EngineChild));
        // The engine stops reading messages for longer than the watchdog
        // allows; the UI thread keeps running and draws nothing meanwhile.
        engine.stall(HUNG_AFTER + Duration::from_secs(2));
        let started = Instant::now();
        child::tests::pump_until(
            || panel.keeper.hung.load(Ordering::Acquire),
            HUNG_AFTER + Duration::from_secs(2),
        );
        assert!(panel.keeper.hung.load(Ordering::Acquire));
        assert!(started.elapsed() < HUNG_AFTER + Duration::from_secs(2));
        let frame = Instant::now();
        assert_eq!(panel.show(slot, Instant::now()), Shown::Frames);
        assert!(frame.elapsed() < Duration::from_millis(50));
        assert_eq!(
            panel.take_path_change(),
            Some(PanelPath::Frames(Fault::Hung))
        );
        // Once the engine reads messages again, its surface is back home.
        child::tests::pump_until(
            || unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::GetParent(engine.surface as HWND)
                    == engine.main as HWND
            },
            Duration::from_secs(10),
        );
        assert_eq!(
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::GetParent(engine.surface as HWND)
            },
            engine.main as HWND
        );
        drop(panel);
        child::tests::pump_until(|| false, Duration::from_millis(300));
        drop(engine);
        unsafe { DestroyWindow(host) };
    }
}
