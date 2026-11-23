use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::HWND, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
};

// winresource embeds assets/rustemudroid.ico under group icon resource ID 1.
// Windows queries separate icons for the caption and the taskbar / Alt+Tab.
fn window_icons(ctx: &eframe::CreationContext<'_>) -> Option<(HWND, [(usize, HICON); 2])> {
    let RawWindowHandle::Win32(handle) = ctx.window_handle().ok()?.as_raw() else {
        return None;
    };
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let load = |width, height| {
            LoadImageW(
                module,
                std::ptr::without_provenance(1),
                IMAGE_ICON,
                GetSystemMetrics(width),
                GetSystemMetrics(height),
                LR_SHARED,
            )
        };
        let icons = [
            (ICON_SMALL as usize, load(SM_CXSMICON, SM_CYSMICON)),
            (ICON_BIG as usize, load(SM_CXICON, SM_CYICON)),
        ];
        icons
            .iter()
            .all(|(_, icon)| !icon.is_null())
            .then_some((handle.hwnd.get() as HWND, icons))
    }
}

pub(super) fn install(ctx: &eframe::CreationContext<'_>) {
    if let Some((hwnd, icons)) = window_icons(ctx) {
        for (size, icon) in icons {
            // The shared resource handles remain valid for the life of the module.
            unsafe {
                SendMessageW(hwnd, WM_SETICON, size, icon as isize);
            }
        }
    }
}

#[cfg(test)]
pub(super) fn matches_brand(ctx: &eframe::CreationContext<'_>) -> bool {
    window_icons(ctx).is_some_and(|(hwnd, icons)| {
        icons
            .into_iter()
            .all(|(size, icon)| unsafe { SendMessageW(hwnd, WM_GETICON, size, 0) == icon as isize })
    })
}
