//! A live DWM relationship between two independent top-level windows.

use std::{
    fmt, io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, SIZE, WAIT_TIMEOUT},
    Graphics::{
        Dwm::{
            DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE,
            DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE, DWMWA_CLOAKED, DwmGetWindowAttribute,
            DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
            DwmUpdateThumbnailProperties,
        },
        Gdi::ClientToScreen,
    },
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW, WaitForSingleObject,
    },
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            SetThreadDpiAwarenessContext,
        },
        WindowsAndMessaging::{
            EnumChildWindows, EnumWindows, GA_ROOT, GW_OWNER, GetAncestor, GetClassNameW,
            GetClientRect, GetWindow, GetWindowThreadProcessId, IsChild, IsIconic, IsWindowVisible,
        },
    },
};

use super::player_process::process_creation_time;

#[derive(Debug)]
pub(crate) enum PreviewError {
    Win32(&'static str, io::Error),
    Dwm(&'static str, i32),
    InvalidDestination,
    InvalidPlayer,
    PlayerIdentityChanged,
    PlayerUnavailable,
    NoSource,
    AmbiguousSource,
    SourceUnavailable,
    InvalidCrop,
    EmptySlot,
    Detached,
}

impl fmt::Display for PreviewError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32(api, error) => write!(out, "{api}: {error}"),
            Self::Dwm(api, code) => write!(out, "{api}: HRESULT 0x{:08X}", *code as u32),
            Self::InvalidDestination => out.write_str("The preview window is unavailable"),
            Self::InvalidPlayer => out.write_str("The selected process is not HD-Player.exe"),
            Self::PlayerIdentityChanged => {
                out.write_str("The verified BlueStacks player process changed")
            }
            Self::PlayerUnavailable => out.write_str("The selected player process has exited"),
            Self::NoSource => out.write_str("The player has no visible BlueStacksApp surface"),
            Self::AmbiguousSource => {
                out.write_str("The player has multiple visible BlueStacksApp surfaces")
            }
            Self::SourceUnavailable => out.write_str("The selected player surface is unavailable"),
            Self::InvalidCrop => out.write_str("The player surface is outside its DWM source"),
            Self::EmptySlot => out.write_str("The preview area has no client pixels"),
            Self::Detached => out.write_str("The DWM thumbnail is detached"),
        }
    }
}

impl std::error::Error for PreviewError {}

type Result<T> = std::result::Result<T, PreviewError>;

/// Right and bottom are exclusive. Destination rectangles are physical client pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreviewRect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

impl PreviewRect {
    fn from_rect(rect: RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    fn as_rect(self) -> RECT {
        RECT {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }

    fn dimensions(self) -> (i64, i64) {
        (
            i64::from(self.right) - i64::from(self.left),
            i64::from(self.bottom) - i64::from(self.top),
        )
    }

    fn nonempty(self) -> Option<Self> {
        (self.left < self.right && self.top < self.bottom).then_some(self)
    }

    /// Convert an egui slot already clipped to the destination viewport in logical points.
    pub(crate) fn from_points(min: [f32; 2], max: [f32; 2], pixels_per_point: f32) -> Option<Self> {
        if !pixels_per_point.is_finite() || pixels_per_point <= 0.0 {
            return None;
        }
        let scale = f64::from(pixels_per_point);
        let [x0, y0] = min.map(|value| (f64::from(value) * scale).ceil());
        let [x1, y1] = max.map(|value| (f64::from(value) * scale).floor());
        if ![x0, y0, x1, y1]
            .into_iter()
            .all(|value| (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value))
        {
            return None;
        }
        Self {
            left: x0 as i32,
            top: y0 as i32,
            right: x1 as i32,
            bottom: y1 as i32,
        }
        .nonempty()
    }

    fn intersect(self, other: Self) -> Option<Self> {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
        .nonempty()
    }

    pub(crate) fn fit(self, source: Self) -> Self {
        let (width, height) = self.dimensions();
        let (source_width, source_height) = source.dimensions();
        let scale = (width as f64 / source_width as f64).min(height as f64 / source_height as f64);
        let shown_width = ((source_width as f64 * scale).round() as i64).clamp(1, width);
        let shown_height = ((source_height as f64 * scale).round() as i64).clamp(1, height);
        let left = i64::from(self.left) + (width - shown_width) / 2;
        let top = i64::from(self.top) + (height - shown_height) / 2;
        Self {
            left: left as i32,
            top: top as i32,
            right: (left + shown_width) as i32,
            bottom: (top + shown_height) as i32,
        }
    }

    /// Map a pointer on the fitted image into the currently oriented Android display.
    /// The caller supplies the full guest display size and verifies that it matches the image.
    pub(crate) fn guest_point(
        self,
        position: [f32; 2],
        pixels_per_point: f32,
        guest_size: [u32; 2],
    ) -> Option<[u32; 2]> {
        let [width, height] = guest_size;
        if !pixels_per_point.is_finite() || pixels_per_point <= 0.0 || width == 0 || height == 0 {
            return None;
        }
        let [x, y] = position.map(|value| f64::from(value) * f64::from(pixels_per_point));
        if !(f64::from(self.left)..f64::from(self.right)).contains(&x)
            || !(f64::from(self.top)..f64::from(self.bottom)).contains(&y)
        {
            return None;
        }
        let (shown_width, shown_height) = self.dimensions();
        Some([
            (((x - f64::from(self.left)) * f64::from(width) / shown_width as f64) as u32)
                .min(width - 1),
            (((y - f64::from(self.top)) * f64::from(height) / shown_height as f64) as u32)
                .min(height - 1),
        ])
    }
}

fn source_crop(root: PreviewRect, renderer: PreviewRect, size: SIZE) -> Result<PreviewRect> {
    if root.intersect(renderer) != Some(renderer) {
        return Err(PreviewError::InvalidCrop);
    }
    // Widen first: monitors to the left of the primary screen can have negative coordinates.
    let relative = |edge: i32, origin: i32, bound: i32| {
        let value = i64::from(edge) - i64::from(origin);
        (0..=i64::from(bound))
            .contains(&value)
            .then_some(value as i32)
    };
    let edges = (
        relative(renderer.left, root.left, size.cx),
        relative(renderer.top, root.top, size.cy),
        relative(renderer.right, root.left, size.cx),
        relative(renderer.bottom, root.top, size.cy),
    );
    match edges {
        (Some(left), Some(top), Some(right), Some(bottom)) => Ok(PreviewRect {
            left,
            top,
            right,
            bottom,
        }),
        _ => Err(PreviewError::InvalidCrop),
    }
}

fn last_error(api: &'static str) -> PreviewError {
    PreviewError::Win32(api, io::Error::last_os_error())
}

fn dwm(api: &'static str, code: i32) -> Result<()> {
    if code < 0 {
        Err(PreviewError::Dwm(api, code))
    } else {
        Ok(())
    }
}

struct PhysicalPixels(DPI_AWARENESS_CONTEXT);

impl PhysicalPixels {
    fn enter() -> Result<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            return Err(last_error("SetThreadDpiAwarenessContext"));
        }
        Ok(Self(previous))
    }
}

impl Drop for PhysicalPixels {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

struct Player(OwnedHandle);

impl Player {
    fn open(pid: u32, expected_creation_time: u64, exe_name: &str) -> Result<Self> {
        if pid == 0 || pid == std::process::id() {
            return Err(PreviewError::InvalidPlayer);
        }
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(last_error("OpenProcess"));
        }
        let player = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        player.verify_creation_time(expected_creation_time)?;
        let mut image = vec![0u16; 32768];
        let mut length = image.len() as u32;
        if unsafe {
            QueryFullProcessImageNameW(player.0.as_raw_handle(), 0, image.as_mut_ptr(), &mut length)
        } == 0
        {
            return Err(last_error("QueryFullProcessImageNameW"));
        }
        let image = String::from_utf16_lossy(&image[..length as usize]);
        if !image
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(exe_name))
        {
            return Err(PreviewError::InvalidPlayer);
        }
        player.check()?;
        Ok(player)
    }

    fn verify_creation_time(&self, expected: u64) -> Result<()> {
        let actual = process_creation_time(&self.0)
            .map_err(|error| PreviewError::Win32("GetProcessTimes", error))?;
        if expected == 0 || actual != expected {
            return Err(PreviewError::PlayerIdentityChanged);
        }
        Ok(())
    }

    fn check(&self) -> Result<()> {
        if unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } != WAIT_TIMEOUT {
            return Err(PreviewError::PlayerUnavailable);
        }
        Ok(())
    }
}

fn window_pid(window: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    pid
}

fn is_root(window: HWND) -> bool {
    !window.is_null() && unsafe { GetAncestor(window, GA_ROOT) } == window
}

fn is_player_root(window: HWND, pid: u32) -> bool {
    window_pid(window) == pid
        && is_root(window)
        && unsafe { GetWindow(window, GW_OWNER) }.is_null()
        && unsafe { IsWindowVisible(window) != 0 && IsIconic(window) == 0 }
}

fn is_renderer(window: HWND, pid: u32, class_name: &str) -> bool {
    if window_pid(window) != pid || unsafe { IsWindowVisible(window) } == 0 {
        return false;
    }
    let mut class = [0u16; 32];
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) } as usize;
    class
        .get(..length)
        .is_some_and(|name| name.iter().copied().eq(class_name.encode_utf16()))
}

fn find_source(pid: u32, class_name: &'static str) -> Result<(HWND, HWND)> {
    struct Search {
        pid: u32,
        class_name: &'static str,
        root: HWND,
        pairs: Vec<(HWND, HWND)>,
    }
    unsafe extern "system" fn child(window: HWND, context: LPARAM) -> i32 {
        // Enumeration is synchronous and context points to our live Search.
        let search = unsafe { &mut *(context as *mut Search) };
        if is_renderer(window, search.pid, search.class_name) {
            search.pairs.push((search.root, window));
        }
        1
    }
    unsafe extern "system" fn root(window: HWND, context: LPARAM) -> i32 {
        let search = unsafe { &mut *(context as *mut Search) };
        if is_player_root(window, search.pid) {
            search.root = window;
            unsafe { EnumChildWindows(window, Some(child), context) };
        }
        1
    }
    let mut search = Search {
        pid,
        class_name,
        root: ptr::null_mut(),
        pairs: Vec::new(),
    };
    if unsafe { EnumWindows(Some(root), (&mut search as *mut Search) as LPARAM) } == 0 {
        return Err(last_error("EnumWindows"));
    }
    match search.pairs.as_slice() {
        [pair] => Ok(*pair),
        [] => Err(PreviewError::NoSource),
        _ => Err(PreviewError::AmbiguousSource),
    }
}

fn client_rect(window: HWND) -> Result<PreviewRect> {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(window, &mut rect) } == 0 {
        return Err(last_error("GetClientRect"));
    }
    Ok(PreviewRect::from_rect(rect))
}

fn screen_client_rect(window: HWND) -> Result<PreviewRect> {
    let client = client_rect(window)?;
    let mut a = POINT {
        x: client.left,
        y: client.top,
    };
    let mut b = POINT {
        x: client.right,
        y: client.bottom,
    };
    if unsafe { ClientToScreen(window, &mut a) == 0 || ClientToScreen(window, &mut b) == 0 } {
        return Err(last_error("ClientToScreen"));
    }
    Ok(PreviewRect {
        left: a.x,
        top: a.y,
        right: b.x,
        bottom: b.y,
    })
}

fn is_cloaked(window: HWND) -> Result<bool> {
    let mut flags = 0u32;
    dwm("DwmGetWindowAttribute", unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED as u32,
            (&mut flags as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    })?;
    Ok(flags != 0)
}

pub(crate) struct ThumbnailPreview {
    destination: HWND,
    source: HWND,
    renderer: HWND,
    renderer_class: &'static str,
    player: Player,
    player_pid: u32,
    thumbnail: isize,
    shown: Option<(PreviewRect, PreviewRect)>,
}

impl ThumbnailPreview {
    /// Bind to a verified, separately running HD-Player. Call on the destination UI thread.
    pub(crate) fn attach(
        destination: HWND,
        player_pid: u32,
        expected_creation_time: u64,
    ) -> Result<Self> {
        Self::attach_surface(
            destination,
            player_pid,
            expected_creation_time,
            "HD-Player.exe",
            "BlueStacksApp",
        )
    }

    #[cfg(test)]
    pub(crate) fn attach_qt_emulator(destination: HWND, pid: u32, creation: u64) -> Result<Self> {
        Self::attach_surface(
            destination,
            pid,
            creation,
            "qemu-system-x86_64.exe",
            "subWin",
        )
    }

    #[cfg(test)]
    pub(crate) fn lab_windows(&self) -> (HWND, HWND, HWND) {
        (self.destination, self.source, self.renderer)
    }

    fn attach_surface(
        destination: HWND,
        player_pid: u32,
        expected_creation_time: u64,
        exe_name: &str,
        renderer_class: &'static str,
    ) -> Result<Self> {
        let _dpi = PhysicalPixels::enter()?;
        if !is_root(destination) || window_pid(destination) != std::process::id() {
            return Err(PreviewError::InvalidDestination);
        }
        let player = Player::open(player_pid, expected_creation_time, exe_name)?;
        let (source, renderer) = find_source(player_pid, renderer_class)?;
        if is_cloaked(source)? {
            return Err(PreviewError::SourceUnavailable);
        }
        player.check()?;
        let mut thumbnail = 0;
        dwm("DwmRegisterThumbnail", unsafe {
            DwmRegisterThumbnail(destination, source, &mut thumbnail)
        })?;
        if thumbnail == 0 {
            return Err(PreviewError::Detached);
        }
        let preview = Self {
            destination,
            source,
            renderer,
            renderer_class,
            player,
            player_pid,
            thumbnail,
            shown: None,
        };
        preview.player.check()?;
        Ok(preview)
    }

    /// Show the aspect-fitted image and return the exact client rectangle for pointer hit testing.
    /// Retry after a minimized window is restored. Drop and reattach if its renderer was replaced.
    pub(crate) fn position(&mut self, slot: PreviewRect) -> Result<PreviewRect> {
        let result = self.show(slot);
        if result.is_err() {
            let _ = self.hide();
        }
        result
    }

    fn show(&mut self, slot: PreviewRect) -> Result<PreviewRect> {
        if self.thumbnail == 0 {
            return Err(PreviewError::Detached);
        }
        let _dpi = PhysicalPixels::enter()?;
        self.player.check()?;
        if !is_root(self.destination)
            || window_pid(self.destination) != std::process::id()
            || unsafe { IsWindowVisible(self.destination) == 0 || IsIconic(self.destination) != 0 }
            || is_cloaked(self.destination)?
        {
            return Err(PreviewError::InvalidDestination);
        }
        if !is_player_root(self.source, self.player_pid)
            || !is_renderer(self.renderer, self.player_pid, self.renderer_class)
            || unsafe { IsChild(self.source, self.renderer) } == 0
            || is_cloaked(self.source)?
        {
            return Err(PreviewError::SourceUnavailable);
        }
        let slot = slot
            .intersect(client_rect(self.destination)?)
            .ok_or(PreviewError::EmptySlot)?;
        let mut size = SIZE::default();
        dwm("DwmQueryThumbnailSourceSize", unsafe {
            DwmQueryThumbnailSourceSize(self.thumbnail, &mut size)
        })?;
        let crop = source_crop(
            screen_client_rect(self.source)?,
            screen_client_rect(self.renderer)?,
            size,
        )?;
        let destination = slot.fit(crop);
        if self.shown != Some((crop, destination)) {
            let properties = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION
                    | DWM_TNP_RECTSOURCE
                    | DWM_TNP_OPACITY
                    | DWM_TNP_VISIBLE
                    | DWM_TNP_SOURCECLIENTAREAONLY,
                rcDestination: destination.as_rect(),
                rcSource: crop.as_rect(),
                opacity: 255,
                fVisible: 1,
                fSourceClientAreaOnly: 1,
            };
            dwm("DwmUpdateThumbnailProperties", unsafe {
                DwmUpdateThumbnailProperties(self.thumbnail, &properties)
            })?;
            self.shown = Some((crop, destination));
        }
        Ok(destination)
    }

    /// Hide while the UI slot is covered, inactive, or too small.
    pub(crate) fn hide(&mut self) -> Result<()> {
        if self.shown.is_none() {
            return Ok(());
        }
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_VISIBLE,
            fVisible: 0,
            ..Default::default()
        };
        match dwm("DwmUpdateThumbnailProperties", unsafe {
            DwmUpdateThumbnailProperties(self.thumbnail, &properties)
        }) {
            Ok(()) => {
                self.shown = None;
                Ok(())
            }
            Err(error) => {
                let _ = self.unregister();
                Err(error)
            }
        }
    }

    pub(crate) fn detach(mut self) -> Result<()> {
        self.unregister()
    }

    fn unregister(&mut self) -> Result<()> {
        if self.thumbnail != 0 {
            dwm("DwmUnregisterThumbnail", unsafe {
                DwmUnregisterThumbnail(self.thumbnail)
            })?;
            self.shown = None;
            self.thumbnail = 0;
        }
        Ok(())
    }
}

impl Drop for ThumbnailPreview {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_player_handle_rejects_a_stale_creation_time() {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                std::process::id(),
            )
        };
        assert!(!handle.is_null());
        let player = Player(unsafe { OwnedHandle::from_raw_handle(handle) });
        let created = process_creation_time(&player.0).unwrap();
        player.verify_creation_time(created).unwrap();
        for different in [0, created + 1] {
            assert!(matches!(
                player.verify_creation_time(different),
                Err(PreviewError::PlayerIdentityChanged)
            ));
        }
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PreviewRect {
        PreviewRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn recorded_pie64_crop_uses_the_root_client_origin() {
        let root = rect(-1920, 54, 70, 1021);
        let renderer = rect(-1622, 87, 37, 1020);
        let size = SIZE { cx: 1990, cy: 967 };
        let crop = source_crop(root, renderer, size).unwrap();
        assert_eq!(crop, rect(298, 33, 1957, 966));
        assert_eq!(rect(12, 68, 842, 535).fit(crop), rect(12, 68, 842, 535));
    }

    #[test]
    fn crop_rejects_partial_or_empty_render_surfaces_and_old_source_sizes() {
        let root = rect(7, 10, 31, 34);
        let size = SIZE { cx: 24, cy: 24 };
        assert!(source_crop(root, rect(6, 10, 20, 25), size).is_err());
        assert!(source_crop(root, rect(7, 10, 7, 30), size).is_err());
        assert!(source_crop(root, rect(7, 10, 30, 34), SIZE { cx: 20, cy: 24 }).is_err());
    }

    #[test]
    fn egui_slot_is_converted_to_client_pixels_and_letterboxed() {
        let slot = PreviewRect::from_points([10.2, 6.3], [400.9, 306.8], 1.25).unwrap();
        assert_eq!(slot, rect(13, 8, 501, 383));
        let image = slot.fit(rect(0, 0, 1600, 900));
        assert_eq!(image, rect(13, 58, 501, 333));
        assert_eq!(
            image.guest_point([205.6, 156.4], 1.25, [1600, 900]),
            Some([800, 449])
        );
        assert_eq!(image.guest_point([10.0, 156.4], 1.25, [1600, 900]), None);
        assert_eq!(image.guest_point([205.6, 270.0], 1.25, [1600, 900]), None);
        assert_eq!(
            PreviewRect::from_points([f32::NAN, 0.0], [1.0, 1.0], 1.25),
            None
        );
    }
}
