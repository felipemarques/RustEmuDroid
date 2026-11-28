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
//! When the engine's window stops answering, the client detaches the queues
//! at once (that call does not wait for the engine's thread) and gives the
//! surface back from a thread of its own; nothing here waits for the
//! engine's thread while it does not answer.

use super::{Fault, window_rect};
use crate::bluestacks::preview::PreviewRect;
use std::{ptr, thread};
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{BLACK_BRUSH, GetStockObject, MapWindowPoints},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{AttachThreadInput, GetCurrentThreadId},
    },
    UI::{
        HiDpi::{GetAwarenessFromDpiAwarenessContext, GetWindowDpiAwarenessContext},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetParent,
            GetWindowThreadProcessId, HTTRANSPARENT, HWND_TOP, IsWindow, IsWindowVisible,
            MA_NOACTIVATE, PostMessageW, RegisterClassW, SW_HIDE, SWP_ASYNCWINDOWPOS,
            SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetParent, SetWindowPos,
            ShowWindow, ShowWindowAsync, WM_CLOSE, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSW,
            WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_DISABLED, WS_EX_NOACTIVATE,
            WS_EX_NOPARENTNOTIFY,
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
        // WM_CLOSE (posted after a surface was given back from another
        // thread) destroys it on this thread, as DefWindowProc does.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

pub(super) struct Holder {
    window: isize,
    /// Where it was put last (physical client pixels of the client's window).
    placed: Option<PreviewRect>,
    /// The render surface it holds, the engine window to give it back to,
    /// and the thread of both.
    surface: Option<(isize, isize, u32)>,
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

    /// The surface held, while it is still a child of this window.
    pub(super) fn held(&mut self) -> Option<isize> {
        let (surface, ..) = self.surface?;
        let alive = unsafe {
            IsWindow(surface as HWND) != 0 && GetParent(surface as HWND) == self.window as HWND
        };
        if !alive {
            self.surface = None;
        }
        alive.then_some(surface)
    }

    /// Makes the engine's render surface `surface`, found in the engine's
    /// window `main`, a child of this window.
    pub(super) fn adopt(&mut self, surface: isize, main: isize) -> Result<(), Fault> {
        let (surface_thread, main_thread) = unsafe {
            (
                GetWindowThreadProcessId(surface as HWND, ptr::null_mut()),
                GetWindowThreadProcessId(main as HWND, ptr::null_mut()),
            )
        };
        // SetParent waits for the surface's thread: only a surface of the
        // thread that runs the engine's window (and reads its messages) is taken.
        if surface_thread == 0 || surface_thread != main_thread {
            return Err(Fault::Lost(
                "The render surface does not belong to the thread of the engine window".into(),
            ));
        }
        let previous = unsafe { SetParent(surface as HWND, self.window as HWND) };
        if previous.is_null() {
            return Err(Fault::System(format!(
                "SetParent: {}",
                std::io::Error::last_os_error()
            )));
        }
        self.surface = Some((surface, main, surface_thread));
        if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
            let awareness = |window: isize| unsafe {
                GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(window as HWND))
            };
            println!(
                "engine_child adopted surface=0x{surface:x} from=0x{main:x} dpi_awareness surface={} panel={}",
                awareness(surface),
                awareness(self.window)
            );
        }
        Ok(())
    }

    /// Detaches the input queue of the client's thread from the engine's
    /// thread; `true` when they were joined.
    pub(super) fn detach(&mut self) -> bool {
        let Some((.., thread)) = self.surface else {
            return false;
        };
        let detached = unsafe { AttachThreadInput(GetCurrentThreadId(), thread, 0) } != 0;
        if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
            println!("engine_child input_detached={detached} error={}", unsafe {
                GetLastError()
            });
        }
        detached
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

    /// Gives the render surface back to the engine's window: at once when the
    /// engine answers, otherwise from a thread of its own, after which this
    /// window is destroyed on its own thread.
    pub(super) fn release(mut self, answering: bool) {
        let Some((surface, main, _)) = self.surface.take() else {
            return;
        };
        if unsafe { IsWindow(surface as HWND) } == 0 {
            return;
        }
        if answering {
            give_back(surface, main);
            return;
        }
        self.hide();
        let holder = self.window;
        // The window must outlive the surface it holds: destroying it now
        // would wait for the engine's thread.
        std::mem::forget(self);
        let _ = thread::Builder::new()
            .name("engine surface release".into())
            .spawn(move || {
                give_back(surface, main);
                unsafe { PostMessageW(holder as HWND, WM_CLOSE, 0, 0) };
            });
    }
}

fn give_back(surface: isize, main: isize) {
    unsafe {
        if IsWindow(main as HWND) != 0 {
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
        if let Some((surface, main, _)) = self.surface.take()
            && unsafe { IsWindow(surface as HWND) } != 0
        {
            give_back(surface, main);
        }
        unsafe { DestroyWindow(self.window as HWND) };
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
mod tests {
    use super::*;

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

    #[test]
    fn a_holder_window_is_created_hidden_inside_its_parent_and_placed_there() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CW_USEDEFAULT, WS_OVERLAPPEDWINDOW};
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let parent = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                ptr::null_mut(),
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        };
        assert!(!parent.is_null());
        let mut holder = Holder::create(parent).unwrap();
        let window = holder.window as HWND;
        assert_eq!(unsafe { GetParent(window) }, parent);
        assert_eq!(unsafe { IsWindowVisible(window) }, 0);
        assert_eq!(holder.held(), None);
        // No surface: nothing to detach.
        assert!(!holder.detach());
        holder.place(rect(10, 20, 330, 200));
        assert_eq!(
            window_rect(window).map(|r| (r.right - r.left, r.bottom - r.top)),
            Some((320, 180))
        );
        holder.hide();
        assert_eq!(unsafe { IsWindowVisible(window) }, 0);
        drop(holder);
        assert_eq!(unsafe { IsWindow(window) }, 0);
        unsafe { DestroyWindow(parent) };
    }
}
