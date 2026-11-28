//! Child mode: a child window of the client over the panel that holds the
//! engine's render surface, so the engine presents inside the client's window.
//!
//! The surface is the engine's own `subWin` (a disabled child of its Qt
//! window, presented by gfxstream's swapchain). It is moved into this window
//! with `SetParent`; the engine keeps sizing it, and this window keeps it
//! centred. The client's drawing over the panel is cut out of this window's
//! region, which clips the surface as well.

use super::{Fault, window_rect};
use crate::bluestacks::preview::PreviewRect;
use std::ptr;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BLACK_BRUSH, CombineRgn, CreateRectRgn, DeleteObject, GetStockObject, MapWindowPoints,
        RGN_DIFF, SetWindowRgn,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::{GetAwarenessFromDpiAwarenessContext, GetWindowDpiAwarenessContext},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetParent,
            GetWindowThreadProcessId, HTTRANSPARENT, HWND_TOP, IsWindow, IsWindowVisible,
            MA_NOACTIVATE, RegisterClassW, SW_HIDE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE,
            SWP_NOZORDER, SWP_SHOWWINDOW, SetParent, SetWindowPos, ShowWindow, ShowWindowAsync,
            WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
            WS_DISABLED, WS_EX_NOACTIVATE, WS_EX_NOPARENTNOTIFY,
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
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

pub(super) struct Holder {
    window: isize,
    /// Where it was put last (physical client pixels of the client's window).
    placed: Option<PreviewRect>,
    /// The parts of the panel left to the client's own drawing, same pixels.
    cut: Vec<PreviewRect>,
    /// The render surface it holds, and the engine window to give it back to.
    surface: Option<(isize, isize)>,
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
                cut: Vec::new(),
                surface: None,
            })
        }
    }

    pub(super) fn place(&mut self, slot: PreviewRect) {
        let window = self.window as HWND;
        if self.placed == Some(slot) && unsafe { IsWindowVisible(window) } != 0 {
            return;
        }
        if self.placed.is_some_and(|placed| placed != slot) {
            // The cut-outs were relative to the old place.
            self.apply_cut(Vec::new(), slot);
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
        let (surface, _) = self.surface?;
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
        self.surface = Some((surface, main));
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

    /// Leaves `cut` (physical client pixels of the client's window) to the
    /// client's own drawing.
    pub(super) fn apply_cut(&mut self, cut: Vec<PreviewRect>, slot: PreviewRect) {
        if cut == self.cut {
            return;
        }
        let window = self.window as HWND;
        unsafe {
            if cut.is_empty() {
                SetWindowRgn(window, ptr::null_mut(), 1);
            } else {
                let region = CreateRectRgn(0, 0, slot.right - slot.left, slot.bottom - slot.top);
                for part in &cut {
                    let hole = CreateRectRgn(
                        part.left - slot.left,
                        part.top - slot.top,
                        part.right - slot.left,
                        part.bottom - slot.top,
                    );
                    CombineRgn(region, region, hole, RGN_DIFF);
                    DeleteObject(hole);
                }
                // On success the system owns the region.
                if SetWindowRgn(window, region, 1) == 0 {
                    DeleteObject(region);
                }
            }
        }
        self.cut = cut;
    }

    /// Gives the render surface back to the engine's window.
    pub(super) fn release(&mut self) {
        let Some((surface, main)) = self.surface.take() else {
            return;
        };
        unsafe {
            if IsWindow(surface as HWND) == 0 {
                return;
            }
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
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.release();
        unsafe { DestroyWindow(self.window as HWND) };
    }
}

/// The parts of `slot` that `shapes` cover, as whole pixels, merged where they
/// overlap a lot so the region stays small.
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
    fn only_what_is_drawn_over_the_panel_is_cut_out_of_it() {
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
    fn a_holder_window_is_created_hidden_inside_its_parent_and_cut_without_a_surface() {
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
        let slot = rect(10, 20, 330, 200);
        holder.place(slot);
        holder.apply_cut(vec![rect(20, 30, 60, 50)], slot);
        assert_eq!(holder.cut, [rect(20, 30, 60, 50)]);
        // A new place drops the cut-outs, which were relative to the old one.
        holder.place(rect(0, 0, 320, 180));
        assert!(holder.cut.is_empty());
        holder.hide();
        assert_eq!(unsafe { IsWindowVisible(window) }, 0);
        drop(holder);
        assert_eq!(unsafe { IsWindow(window) }, 0);
        unsafe { DestroyWindow(parent) };
    }
}
