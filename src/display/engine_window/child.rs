//! Child mode: a child window of the client over the panel that holds the
//! engine's render surface, so the engine presents inside the client's window.
//!
//! The surface is the engine's own `subWin` (a disabled child of its Qt
//! window, presented by gfxstream's swapchain). It is moved into this window
//! with `SetParent`; the engine keeps sizing it, and this window keeps it
//! centred. What the client draws over the panel is shown above it by the
//! overlay window.
//!
//! A child window of another thread joins the input queues of the two threads,
//! as `AttachThreadInput` does. That is what sends a click on the disabled
//! surface to the client's window (detached, the click goes to the engine's
//! thread and is lost), so the queues stay joined while the surface is held.
//!
//! `SetParent` on the engine's surface waits for the engine's thread, so the
//! client's UI thread never calls it: the surface is taken and given back by
//! threads of their own, and this window is destroyed only once it no longer
//! holds the engine's surface (destroying it would destroy that too). The
//! watchdog (`keeper`) detaches the queues and gives the surface back when
//! the engine stops answering.

use super::{Fault, Lost, window_rect};
use crate::bluestacks::preview::PreviewRect;
use std::{
    ptr,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{BLACK_BRUSH, GetStockObject, MapWindowPoints},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::{GetAwarenessFromDpiAwarenessContext, GetWindowDpiAwarenessContext},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GW_CHILD, GetClientRect, GetParent,
            GetWindow, GetWindowThreadProcessId, HTTRANSPARENT, HWND_TOP, IsWindow,
            IsWindowVisible, MA_NOACTIVATE, PostMessageW, RegisterClassW, SW_HIDE,
            SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
            SetParent, SetWindowPos, ShowWindow, ShowWindowAsync, WM_CLOSE, WM_MOUSEACTIVATE,
            WM_NCHITTEST, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_DISABLED,
            WS_EX_NOACTIVATE, WS_EX_NOPARENTNOTIFY,
        },
    },
};

unsafe extern "system" fn holder_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        // The pointer belongs to the panel under it. The window is disabled
        // too, which is what sends clicks on the engine's surface there.
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        // WM_CLOSE, posted once the surface is given back, destroys it on
        // this thread, as DefWindowProc does.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// A surface being taken by a thread of its own: its result, once known.
type Taking = Arc<Mutex<Option<Result<(), Fault>>>>;

pub(super) struct Holder {
    window: isize,
    /// Where it was put last (physical client pixels of the client's window).
    placed: Option<PreviewRect>,
    /// The render surface it holds (or is taking) and the engine window to
    /// give it back to.
    surface: Option<(isize, isize)>,
    taking: Option<(Taking, thread::JoinHandle<()>)>,
}

impl Holder {
    pub(super) fn create(host: HWND) -> Result<Self, Fault> {
        let class: Vec<u16> = "RustEmuDroidEngineChild\0".encode_utf16().collect();
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let definition = WNDCLASSW {
                lpfnWndProc: Some(holder_proc),
                hInstance: instance,
                hbrBackground: GetStockObject(BLACK_BRUSH),
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // A second session in this process finds the class registered.
            RegisterClassW(&definition);
            let window = CreateWindowExW(
                WS_EX_NOPARENTNOTIFY | WS_EX_NOACTIVATE,
                class.as_ptr(),
                ptr::null(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_DISABLED,
                0,
                0,
                1,
                1,
                host,
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if window.is_null() {
                return Err(Fault::System(format!(
                    "CreateWindowExW: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(Self {
                window: window as isize,
                placed: None,
                surface: None,
                taking: None,
            })
        }
    }

    pub(super) fn place(&mut self, slot: PreviewRect) {
        let window = self.window as HWND;
        if self.placed == Some(slot) && unsafe { IsWindowVisible(window) } != 0 {
            return;
        }
        unsafe {
            SetWindowPos(
                window,
                HWND_TOP,
                slot.left,
                slot.top,
                slot.right - slot.left,
                slot.bottom - slot.top,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
        self.placed = Some(slot);
    }

    pub(super) fn hide(&mut self) {
        // Cheap when it is hidden already.
        unsafe { ShowWindow(self.window as HWND, SW_HIDE) };
    }

    /// The surface held, while it is a child of this window; `Err` when
    /// taking it failed.
    pub(super) fn held(&mut self) -> Result<Option<isize>, Fault> {
        if let Some((result, _)) = &self.taking {
            let outcome = result.lock().unwrap().take();
            match outcome {
                None => return Ok(None),
                Some(Err(fault)) => {
                    self.taking = None;
                    self.surface = None;
                    return Err(fault);
                }
                Some(Ok(())) => self.taking = None,
            }
        }
        let Some((surface, _)) = self.surface else {
            return Ok(None);
        };
        let alive = unsafe {
            IsWindow(surface as HWND) != 0 && GetParent(surface as HWND) == self.window as HWND
        };
        if !alive {
            self.surface = None;
        }
        Ok(alive.then_some(surface))
    }

    /// Whether a surface is on its way into this window.
    pub(super) fn taking(&self) -> bool {
        self.taking.is_some()
    }

    /// Starts making the engine's render surface `surface`, found in the
    /// engine's window `main`, a child of this window, from a thread of its
    /// own; `held` tells when it is.
    pub(super) fn adopt(&mut self, surface: isize, main: isize) -> Result<(), Fault> {
        let (surface_thread, main_thread) = unsafe {
            (
                GetWindowThreadProcessId(surface as HWND, ptr::null_mut()),
                GetWindowThreadProcessId(main as HWND, ptr::null_mut()),
            )
        };
        // Only a surface of the thread that runs the engine's window (and
        // reads its messages) is taken.
        if surface_thread == 0 || surface_thread != main_thread {
            return Err(Fault::Lost(Lost::OtherThread));
        }
        let result: Taking = Arc::default();
        let report = result.clone();
        let holder = self.window;
        let taker = thread::Builder::new()
            .name("engine surface take".into())
            .spawn(move || {
                let previous = unsafe { SetParent(surface as HWND, holder as HWND) };
                let outcome = if previous.is_null() {
                    Err(Fault::System(format!(
                        "SetParent: {}",
                        std::io::Error::last_os_error()
                    )))
                } else {
                    Ok(())
                };
                if outcome.is_ok() && std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                    let awareness = |window: isize| unsafe {
                        GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(
                            window as HWND,
                        ))
                    };
                    println!(
                        "engine_child adopted surface=0x{surface:x} from=0x{main:x} dpi_awareness surface={} panel={}",
                        awareness(surface),
                        awareness(holder)
                    );
                }
                *report.lock().unwrap() = Some(outcome);
            })
            .map_err(|error| Fault::System(format!("CreateThread: {error}")))?;
        self.surface = Some((surface, main));
        self.taking = Some((result, taker));
        Ok(())
    }

    /// Centres the held surface in this window (the engine keeps the aspect
    /// of the Android screen and places it its own way) and returns its size.
    pub(super) fn centre(&self, surface: isize) -> Option<(i32, i32)> {
        let placed = self.placed?;
        let handle = surface as HWND;
        let mut size = RECT::default();
        let mut at = window_rect(handle)?;
        unsafe {
            if GetClientRect(handle, &mut size) == 0 {
                return None;
            }
            MapWindowPoints(
                ptr::null_mut(),
                self.window as HWND,
                (&mut at as *mut RECT).cast(),
                2,
            );
        }
        let (width, height) = (size.right, size.bottom);
        let x = ((placed.right - placed.left) - width) / 2;
        let y = ((placed.bottom - placed.top) - height) / 2;
        if (at.left, at.top) != (x, y) {
            // Asynchronous: the surface belongs to the engine's thread.
            unsafe {
                SetWindowPos(
                    handle,
                    ptr::null_mut(),
                    x,
                    y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                )
            };
        }
        Some((width, height))
    }

    /// Gives the render surface back to the engine's window and destroys
    /// this window once it no longer holds it, without waiting here: from a
    /// thread of its own, which first waits for a surface still being taken.
    /// A surface whose engine window is gone is left to the engine's exit.
    pub(super) fn release(mut self) {
        self.hide();
        let holder = self.window;
        let surface = self.surface.take();
        let taking = self.taking.take().map(|(_, taker)| taker);
        // Nothing of the engine's: destroyed at once (by Drop).
        if surface.is_none() && taking.is_none() && !holds_a_child(holder) {
            return;
        }
        // Otherwise the window must outlive the surface it holds.
        std::mem::forget(self);
        let _ = thread::Builder::new()
            .name("engine surface release".into())
            .spawn(move || {
                if let Some(taker) = taking {
                    let _ = taker.join();
                }
                if let Some((surface, main)) = surface {
                    give_back(surface, main);
                }
                while holds_a_child(holder) {
                    thread::sleep(Duration::from_millis(200));
                }
                // Destroyed on its own thread (DefWindowProc handles WM_CLOSE).
                unsafe { PostMessageW(holder as HWND, WM_CLOSE, 0, 0) };
            });
    }
}

fn holds_a_child(window: isize) -> bool {
    unsafe { IsWindow(window as HWND) != 0 && !GetWindow(window as HWND, GW_CHILD).is_null() }
}

/// Makes the engine's render surface a child of the engine's window `main`
/// again (waits for the engine's thread), or hides it when that is gone.
pub(super) fn give_back(surface: isize, main: isize) {
    unsafe {
        if IsWindow(surface as HWND) == 0 {
            return;
        }
        if main != 0 && IsWindow(main as HWND) != 0 {
            SetParent(surface as HWND, main as HWND);
            SetWindowPos(
                surface as HWND,
                ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        } else {
            ShowWindowAsync(surface as HWND, SW_HIDE);
        }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        // `release` forgets a holder that still holds something of the
        // engine's; this one never destroys the engine's surface.
        if !holds_a_child(self.window) {
            unsafe { DestroyWindow(self.window as HWND) };
        }
    }
}

/// The parts of `slot` that `shapes` cover, as whole pixels, merged where one
/// contains another.
pub(super) fn cut_out(slot: PreviewRect, shapes: &[PreviewRect]) -> Vec<PreviewRect> {
    let mut cut: Vec<PreviewRect> = Vec::new();
    for shape in shapes {
        let part = PreviewRect {
            left: shape.left.max(slot.left),
            top: shape.top.max(slot.top),
            right: shape.right.min(slot.right),
            bottom: shape.bottom.min(slot.bottom),
        };
        if part.right <= part.left || part.bottom <= part.top {
            continue;
        }
        if cut.iter().any(|kept| contains(*kept, part)) {
            continue;
        }
        cut.retain(|kept| !contains(part, *kept));
        cut.push(part);
    }
    cut.sort_by_key(|part| (part.top, part.left, part.bottom, part.right));
    cut
}

/// The share of `slot` that `cut` covers (an upper bound: overlapping parts
/// count twice).
pub(super) fn share(slot: PreviewRect, cut: &[PreviewRect]) -> f32 {
    let area =
        |r: &PreviewRect| ((r.right - r.left).max(0) as f32) * ((r.bottom - r.top).max(0) as f32);
    let whole = area(&slot);
    if whole <= 0.0 {
        return 1.0;
    }
    cut.iter().map(area).sum::<f32>() / whole
}

fn contains(outer: PreviewRect, inner: PreviewRect) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && outer.right >= inner.right
        && outer.bottom >= inner.bottom
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::{sync::mpsc, time::Instant};
    use windows_sys::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::WindowsAndMessaging::{
            CW_USEDEFAULT, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, PostThreadMessageW,
            TranslateMessage, WM_QUIT, WS_OVERLAPPEDWINDOW, WS_POPUP,
        },
    };

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PreviewRect {
        PreviewRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn only_what_is_drawn_over_the_panel_counts_as_covering_it() {
        let slot = rect(100, 50, 900, 500);
        assert!(cut_out(slot, &[]).is_empty());
        // Outside, inside, across an edge, inside another one.
        assert_eq!(
            cut_out(
                slot,
                &[
                    rect(0, 0, 90, 40),
                    rect(200, 100, 260, 130),
                    rect(850, 480, 950, 560),
                    rect(210, 105, 220, 110),
                ]
            ),
            [rect(200, 100, 260, 130), rect(850, 480, 900, 500)]
        );
        assert_eq!(share(slot, &[rect(100, 50, 500, 500)]), 0.5);
        assert_eq!(share(slot, &[]), 0.0);
        // A larger shape replaces the smaller ones it covers.
        assert_eq!(
            cut_out(slot, &[rect(200, 100, 260, 130), rect(150, 90, 300, 200)]),
            [rect(150, 90, 300, 200)]
        );
    }

    pub(in super::super) fn window(style: u32, parent: HWND) -> HWND {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                parent,
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        }
    }

    /// Runs the message loop of the calling thread until `stop` holds.
    pub(in super::super) fn pump_until(mut stop: impl FnMut() -> bool, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !stop() && Instant::now() < deadline {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Windows of another thread of this process, like the engine's: a main
    /// window with a disabled child as its render surface. Its thread can be
    /// told to stop reading messages for a while, as a hung engine does.
    pub(in super::super) struct Engine {
        pub(in super::super) main: isize,
        pub(in super::super) surface: isize,
        pub(in super::super) thread: u32,
        stall: mpsc::Sender<Duration>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl Engine {
        pub(in super::super) fn start() -> Self {
            let (ready, windows) = mpsc::channel();
            let (stall, stalls) = mpsc::channel::<Duration>();
            let handle = thread::spawn(move || {
                let main = window(WS_POPUP, ptr::null_mut());
                let surface = window(WS_CHILD | WS_DISABLED, main);
                let id = unsafe { GetCurrentThreadId() };
                ready.send((main as isize, surface as isize, id)).unwrap();
                loop {
                    if let Ok(pause) = stalls.try_recv() {
                        // Hung: no message is read meanwhile.
                        thread::sleep(pause);
                    }
                    let mut message = MSG::default();
                    unsafe {
                        if PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                            if message.message == WM_QUIT {
                                break;
                            }
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                unsafe { DestroyWindow(main) };
            });
            let (main, surface, thread) = windows.recv().unwrap();
            Self {
                main,
                surface,
                thread,
                stall,
                handle: Some(handle),
            }
        }

        pub(in super::super) fn stall(&self, pause: Duration) {
            self.stall.send(pause).unwrap();
            // The engine's thread takes the request at its next turn.
            thread::sleep(Duration::from_millis(30));
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    #[test]
    fn a_surface_is_taken_and_given_back_without_the_ui_thread_waiting_for_the_engine() {
        let host = window(WS_OVERLAPPEDWINDOW, ptr::null_mut());
        let engine = Engine::start();
        let mut holder = Holder::create(host).unwrap();
        let window = holder.window;
        assert_eq!(unsafe { GetParent(window as HWND) }, host);
        assert_eq!(holder.held(), Ok(None));
        holder.place(rect(10, 20, 330, 200));
        // The engine's thread does not read messages: taking the surface
        // returns at once and the surface arrives once it reads them again.
        engine.stall(Duration::from_millis(500));
        let started = Instant::now();
        holder.adopt(engine.surface, engine.main).unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(holder.taking());
        pump_until(
            || holder.held() == Ok(Some(engine.surface)),
            Duration::from_secs(5),
        );
        assert!(!holder.taking());
        assert_eq!(unsafe { GetParent(engine.surface as HWND) }, window as HWND);
        // Hung again: releasing returns at once, and the holder stays until
        // the surface is back.
        engine.stall(Duration::from_millis(800));
        let started = Instant::now();
        holder.release();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(unsafe { IsWindow(window as HWND) }, 1);
        pump_until(
            || unsafe { IsWindow(window as HWND) } == 0,
            Duration::from_secs(5),
        );
        assert_eq!(unsafe { IsWindow(window as HWND) }, 0);
        assert_eq!(
            unsafe { GetParent(engine.surface as HWND) },
            engine.main as HWND
        );
        unsafe { DestroyWindow(host) };
    }

    #[test]
    fn a_holder_whose_engine_window_is_gone_never_destroys_the_surface() {
        let host = window(WS_OVERLAPPEDWINDOW, ptr::null_mut());
        let engine = Engine::start();
        let mut holder = Holder::create(host).unwrap();
        holder.adopt(engine.surface, engine.main).unwrap();
        pump_until(
            || holder.held() == Ok(Some(engine.surface)),
            Duration::from_secs(5),
        );
        let window = holder.window;
        // No engine window to give the surface back to: it is only hidden,
        // and the holder waits for it to go away with its engine.
        holder.surface = Some((engine.surface, 0));
        holder.release();
        pump_until(|| false, Duration::from_millis(400));
        assert_eq!(unsafe { IsWindow(engine.surface as HWND) }, 1);
        assert_eq!(unsafe { IsWindow(window as HWND) }, 1);
        let surface = engine.surface;
        drop(engine);
        pump_until(
            || unsafe { IsWindow(window as HWND) } == 0,
            Duration::from_secs(5),
        );
        assert_eq!(unsafe { IsWindow(window as HWND) }, 0);
        assert_eq!(unsafe { IsWindow(surface as HWND) }, 0);
        unsafe { DestroyWindow(host) };
    }

    #[test]
    fn a_surface_of_another_thread_than_its_window_is_not_taken() {
        let host = window(WS_OVERLAPPEDWINDOW, ptr::null_mut());
        let engine = Engine::start();
        let other = window(WS_POPUP, ptr::null_mut());
        let mut holder = Holder::create(host).unwrap();
        assert_eq!(
            holder.adopt(engine.surface, other as isize),
            Err(Fault::Lost(Lost::OtherThread))
        );
        drop(holder);
        unsafe {
            DestroyWindow(other);
            DestroyWindow(host);
        }
    }
}
