//! Child mode: what the client draws over the panel, shown on top of the
//! engine's live picture.
//!
//! The engine's render surface is a child window over the panel, so whatever
//! egui paints there in the client's own surface is hidden under it. The
//! shapes of the frame that fall on the panel are tessellated by egui itself,
//! rasterised here on the CPU into a premultiplied bitmap and handed to the
//! compositor through a layered window owned by the client's window and kept
//! exactly over them. That window is transparent to the pointer
//! (`WS_EX_TRANSPARENT`): clicks, drags and the wheel go through it and through
//! the disabled surface to the client's window, where egui handles them as
//! before. It is redrawn only when what is drawn changes, and hidden while
//! nothing is drawn over the panel.

use super::Fault;
use crate::bluestacks::preview::PreviewRect;
use eframe::egui::{
    ColorImage, TextureId,
    epaint::{ClippedPrimitive, Primitive},
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    ptr,
    sync::{Arc, Mutex, OnceLock},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM},
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        ClientToScreen, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SelectObject,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HTTRANSPARENT, IsIconic, IsWindowVisible,
        MA_NOACTIVATE, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE,
        SWP_NOZORDER, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_MOUSEACTIVATE,
        WM_NCHITTEST, WNDCLASSW, WS_DISABLED, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TRANSPARENT, WS_POPUP,
    },
};

/// Textures other than the font atlas that the client may draw over the
/// panel, by the id egui gave them, with their pixels.
fn textures() -> &'static Mutex<HashMap<TextureId, Arc<ColorImage>>> {
    static TEXTURES: OnceLock<Mutex<HashMap<TextureId, Arc<ColorImage>>>> = OnceLock::new();
    TEXTURES.get_or_init(Default::default)
}

/// Makes the pixels of a texture the client loaded available to the overlay
/// (triangles with an unknown texture are left out of it).
pub(crate) fn remember_texture(id: TextureId, image: &ColorImage) {
    textures()
        .lock()
        .unwrap()
        .insert(id, Arc::new(image.clone()));
}

/// The font atlas as the overlay last copied it. egui adds glyphs to its
/// atlas in place and has no version to compare, so a copy is used again
/// only while every glyph the overlay draws was already used when it was
/// made, and the atlas neither grew nor started over.
#[derive(Default)]
pub(crate) struct FontCopy {
    size: [usize; 2],
    fill: f32,
    glyphs: HashSet<[u32; 2]>,
    image: Option<Arc<ColorImage>>,
}

impl FontCopy {
    /// The atlas for `primitives`, copied with `copy` when this one may lack
    /// some of their glyphs. `size` and `fill` describe the atlas now.
    pub(crate) fn get(
        &mut self,
        primitives: &[ClippedPrimitive],
        size: [usize; 2],
        fill: f32,
        copy: impl FnOnce() -> ColorImage,
    ) -> Arc<ColorImage> {
        let glyphs = glyphs(primitives);
        if let Some(image) = &self.image
            && self.size == size
            && fill >= self.fill
            && glyphs.is_subset(&self.glyphs)
        {
            return image.clone();
        }
        let image = Arc::new(copy());
        *self = Self {
            size,
            fill,
            glyphs,
            image: Some(image.clone()),
        };
        image
    }
}

/// Where the font-textured vertices of `primitives` sample the atlas (the
/// white texel of solid shapes aside).
fn glyphs(primitives: &[ClippedPrimitive]) -> HashSet<[u32; 2]> {
    primitives
        .iter()
        .filter_map(|clipped| match &clipped.primitive {
            Primitive::Mesh(mesh) if mesh.texture_id == TextureId::default() => Some(mesh),
            _ => None,
        })
        .flat_map(|mesh| &mesh.vertices)
        .filter(|vertex| vertex.uv != eframe::egui::epaint::WHITE_UV)
        .map(|vertex| [vertex.uv.x.to_bits(), vertex.uv.y.to_bits()])
        .collect()
}

thread_local! {
    /// The overlay window and its offset from the client area's origin, for
    /// the client's window procedure to keep it in place while Windows moves
    /// the client's window (both live on the UI thread).
    static PLACE: Cell<(isize, i32, i32)> = const { Cell::new((0, 0, 0)) };
}

/// Puts the overlay back over the panel after the client's window moved.
pub(super) fn follow(host: HWND) {
    let (window, dx, dy) = PLACE.with(Cell::get);
    if window == 0 || unsafe { IsWindowVisible(window as HWND) } == 0 {
        return;
    }
    let mut origin = POINT { x: 0, y: 0 };
    if unsafe { IsIconic(host) } != 0 || unsafe { ClientToScreen(host, &mut origin) } == 0 {
        return;
    }
    unsafe {
        SetWindowPos(
            window as HWND,
            ptr::null_mut(),
            origin.x + dx,
            origin.y + dy,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

unsafe extern "system" fn overlay_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// A 32-bit top-down DIB section selected into its own memory DC.
struct Bitmap {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    size: (i32, i32),
}

impl Bitmap {
    fn new(width: i32, height: i32) -> Option<Self> {
        unsafe {
            let screen = GetDC(ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            ReleaseDC(ptr::null_mut(), screen);
            if dc.is_null() {
                return None;
            }
            let info = BITMAPINFO {
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
            let mut bits = ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, bitmap);
            Some(Self {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
                size: (width, height),
            })
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: the DIB section holds width * height 32-bit pixels while it lives.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.size.0 * self.size.1) as usize) }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// What the overlay window shows, in physical client pixels of the client's window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Shown {
    pub(super) at: PreviewRect,
    pub(super) fingerprint: u64,
}

pub(super) struct Overlay {
    window: isize,
    bitmap: Option<Bitmap>,
    shown: Option<Shown>,
    /// Updates of the window's picture, for the diagnostics.
    pub(super) redrawn: u64,
}

impl Overlay {
    pub(super) fn create(host: HWND) -> Result<Self, Fault> {
        let class: Vec<u16> = "RustEmuDroidEngineOverlay\0".encode_utf16().collect();
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let definition = WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // A second session in this process finds the class registered.
            RegisterClassW(&definition);
            // Owned by the client's window: always above it, hidden with it
            // when it is minimised, never in the taskbar, never activated,
            // and transparent to the pointer.
            let window = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                ptr::null(),
                WS_POPUP | WS_DISABLED,
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
            PLACE.with(|place| place.set((window as isize, 0, 0)));
            Ok(Self {
                window: window as isize,
                bitmap: None,
                shown: None,
                redrawn: 0,
            })
        }
    }

    pub(super) fn hide(&mut self) {
        if self.shown.take().is_some() {
            unsafe { ShowWindow(self.window as HWND, SW_HIDE) };
        }
    }

    #[cfg(test)]
    pub(super) fn visible(&self) -> bool {
        self.shown.is_some()
    }

    /// Shows `primitives` (tessellated at `scale` pixels per point) over the
    /// part of the client's window `host` they cover inside `slot`. `font`
    /// gives the font atlas, asked for only when the picture must be redrawn.
    pub(super) fn show(
        &mut self,
        host: HWND,
        slot: PreviewRect,
        primitives: &[ClippedPrimitive],
        scale: f32,
        font: impl FnOnce() -> Arc<ColorImage>,
    ) -> Result<(), Fault> {
        let Some(at) = bounds(primitives, scale, slot) else {
            self.hide();
            return Ok(());
        };
        let mut origin = POINT { x: 0, y: 0 };
        if unsafe { ClientToScreen(host, &mut origin) } == 0 {
            self.hide();
            return Ok(());
        }
        let fingerprint = fingerprint(primitives, scale, at);
        let wanted = Shown { at, fingerprint };
        PLACE.with(|place| place.set((self.window, at.left, at.top)));
        if self.shown == Some(wanted) {
            // Same picture at the same place in the client: follow a move.
            follow(host);
            return Ok(());
        }
        let (width, height) = (at.right - at.left, at.bottom - at.top);
        if self
            .bitmap
            .as_ref()
            .is_none_or(|bitmap| bitmap.size != (width, height))
        {
            self.bitmap = None;
            self.bitmap = Some(Bitmap::new(width, height).ok_or_else(|| {
                Fault::System(format!(
                    "CreateDIBSection: {}",
                    std::io::Error::last_os_error()
                ))
            })?);
        }
        let bitmap = self.bitmap.as_mut().unwrap();
        let font = font();
        let known = textures().lock().unwrap();
        let picture = rasterize(
            primitives,
            scale,
            [at.left, at.top],
            [width as usize, height as usize],
            &|id| {
                if id == TextureId::default() {
                    Some(&*font)
                } else {
                    known.get(&id).map(|image| &**image)
                }
            },
        );
        drop(known);
        for (out, [r, g, b, a]) in bitmap.pixels().iter_mut().zip(picture) {
            *out = u32::from_le_bytes([b, g, r, a]);
        }
        let position = POINT {
            x: origin.x + at.left,
            y: origin.y + at.top,
        };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let updated = unsafe {
            UpdateLayeredWindow(
                self.window as HWND,
                ptr::null_mut(),
                &position,
                &size,
                bitmap.dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            )
        } != 0;
        if !updated {
            return Err(Fault::System(format!(
                "UpdateLayeredWindow: {}",
                std::io::Error::last_os_error()
            )));
        }
        if self.shown.is_none() {
            unsafe { ShowWindow(self.window as HWND, SW_SHOWNOACTIVATE) };
        }
        self.shown = Some(wanted);
        self.redrawn += 1;
        Ok(())
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        PLACE.with(|place| place.set((0, 0, 0)));
        self.bitmap = None;
        unsafe { DestroyWindow(self.window as HWND) };
    }
}

/// The physical pixels inside `slot` that `primitives` can touch.
fn bounds(primitives: &[ClippedPrimitive], scale: f32, slot: PreviewRect) -> Option<PreviewRect> {
    let mut found: Option<PreviewRect> = None;
    for clipped in primitives {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            continue;
        }
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for vertex in &mesh.vertices {
            min_x = min_x.min(vertex.pos.x);
            min_y = min_y.min(vertex.pos.y);
            max_x = max_x.max(vertex.pos.x);
            max_y = max_y.max(vertex.pos.y);
        }
        let clip = clipped.clip_rect;
        let part = PreviewRect {
            left: ((min_x.max(clip.min.x)) * scale).floor() as i32,
            top: ((min_y.max(clip.min.y)) * scale).floor() as i32,
            right: ((max_x.min(clip.max.x)) * scale).ceil() as i32,
            bottom: ((max_y.min(clip.max.y)) * scale).ceil() as i32,
        };
        let part = PreviewRect {
            left: part.left.max(slot.left),
            top: part.top.max(slot.top),
            right: part.right.min(slot.right),
            bottom: part.bottom.min(slot.bottom),
        };
        if part.right <= part.left || part.bottom <= part.top {
            continue;
        }
        found = Some(match found {
            None => part,
            Some(so_far) => PreviewRect {
                left: so_far.left.min(part.left),
                top: so_far.top.min(part.top),
                right: so_far.right.max(part.right),
                bottom: so_far.bottom.max(part.bottom),
            },
        });
    }
    found
}

/// Identifies a picture: the same meshes at the same place give the same value.
fn fingerprint(primitives: &[ClippedPrimitive], scale: f32, at: PreviewRect) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scale.to_bits().hash(&mut hasher);
    (at.left, at.top, at.right, at.bottom).hash(&mut hasher);
    for clipped in primitives {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        let clip = clipped.clip_rect;
        [clip.min.x, clip.min.y, clip.max.x, clip.max.y]
            .map(f32::to_bits)
            .hash(&mut hasher);
        mesh.texture_id.hash(&mut hasher);
        mesh.indices.hash(&mut hasher);
        for vertex in &mesh.vertices {
            [vertex.pos.x, vertex.pos.y, vertex.uv.x, vertex.uv.y]
                .map(f32::to_bits)
                .hash(&mut hasher);
            vertex.color.to_array().hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Sub-pixel steps of the rasteriser's fixed-point coordinates.
const SUB: i64 = 256;

/// Paints `primitives` (positions in points, `scale` pixels per point) into a
/// `size` bitmap whose top-left pixel is the physical pixel `origin`, the way
/// egui's own renderer does: premultiplied sRGB colours and texels, blended in
/// gamma space, sampled at pixel centres, each pixel covered by exactly one of
/// two triangles sharing an edge. Returns premultiplied RGBA pixels.
pub(super) fn rasterize<'a>(
    primitives: &[ClippedPrimitive],
    scale: f32,
    origin: [i32; 2],
    size: [usize; 2],
    texture: &dyn Fn(TextureId) -> Option<&'a ColorImage>,
) -> Vec<[u8; 4]> {
    let mut out = vec![[0u8; 4]; size[0] * size[1]];
    for clipped in primitives {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        let Some(image) = texture(mesh.texture_id) else {
            continue;
        };
        let clip = clipped.clip_rect;
        // The clip rectangle in whole pixels of the bitmap, as a scissor would.
        let clip_px = [
            ((clip.min.x * scale).round() as i64 - i64::from(origin[0])).max(0),
            ((clip.min.y * scale).round() as i64 - i64::from(origin[1])).max(0),
            ((clip.max.x * scale).round() as i64 - i64::from(origin[0])).min(size[0] as i64),
            ((clip.max.y * scale).round() as i64 - i64::from(origin[1])).min(size[1] as i64),
        ];
        if clip_px[2] <= clip_px[0] || clip_px[3] <= clip_px[1] {
            continue;
        }
        let point = |index: u32| {
            let vertex = &mesh.vertices[index as usize];
            (
                ((vertex.pos.x * scale - origin[0] as f32) * SUB as f32).round() as i64,
                ((vertex.pos.y * scale - origin[1] as f32) * SUB as f32).round() as i64,
                vertex,
            )
        };
        for triangle in mesh.indices.chunks_exact(3) {
            let mut corners = [point(triangle[0]), point(triangle[1]), point(triangle[2])];
            let area = edge(corners[0], corners[1], (corners[2].0, corners[2].1));
            if area == 0 {
                continue;
            }
            if area < 0 {
                corners.swap(1, 2);
            }
            fill(&mut out, size, clip_px, &corners, area.abs(), image);
        }
    }
    out
}

type Corner<'a> = (i64, i64, &'a eframe::egui::epaint::Vertex);

fn edge(a: Corner, b: Corner, p: (i64, i64)) -> i64 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// Whether a pixel centre exactly on the edge from `a` to `b` belongs to the
/// triangle: true for exactly one of the two directions of any edge.
fn owns_edge(a: Corner, b: Corner) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    dy > 0 || (dy == 0 && dx < 0)
}

fn fill(
    out: &mut [[u8; 4]],
    size: [usize; 2],
    clip: [i64; 4],
    corners: &[Corner; 3],
    area: i64,
    image: &ColorImage,
) {
    let [a, b, c] = *corners;
    let min_x = (a.0.min(b.0).min(c.0).div_euclid(SUB)).max(clip[0]);
    let max_x = ((a.0.max(b.0).max(c.0) + SUB - 1).div_euclid(SUB)).min(clip[2]);
    let min_y = (a.1.min(b.1).min(c.1).div_euclid(SUB)).max(clip[1]);
    let max_y = ((a.1.max(b.1).max(c.1) + SUB - 1).div_euclid(SUB)).min(clip[3]);
    if max_x <= min_x || max_y <= min_y {
        return;
    }
    // Edge i is opposite corner i; its value is corner i's weight times `area`.
    let edges = [(b, c), (c, a), (a, b)];
    let owned = edges.map(|(from, to)| owns_edge(from, to));
    let colours = [a.2, b.2, c.2].map(|vertex| vertex.color.to_array().map(f32::from));
    let uvs = [a.2, b.2, c.2].map(|vertex| vertex.uv);
    // A solid triangle samples one texel (egui's white one, usually).
    let flat = uvs[0] == uvs[1] && uvs[1] == uvs[2];
    let flat_texel = flat.then(|| sample(image, uvs[0].x, uvs[0].y));
    let inverse = 1.0 / area as f32;
    let start = (min_x * SUB + SUB / 2, min_y * SUB + SUB / 2);
    let step_x = edges.map(|(from, to)| -(to.1 - from.1) * SUB);
    let step_y = edges.map(|(from, to)| (to.0 - from.0) * SUB);
    let mut row = edges.map(|(from, to)| edge(from, to, start));
    for y in min_y..max_y {
        let mut weights = row;
        for x in min_x..max_x {
            let inside = (0..3).all(|i| weights[i] > 0 || (weights[i] == 0 && owned[i]));
            if inside {
                let lambda = weights.map(|w| w as f32 * inverse);
                let mut colour = [0f32; 4];
                for (channel, value) in colour.iter_mut().enumerate() {
                    *value = lambda[0] * colours[0][channel]
                        + lambda[1] * colours[1][channel]
                        + lambda[2] * colours[2][channel];
                }
                let texel = flat_texel.unwrap_or_else(|| {
                    let u = lambda[0] * uvs[0].x + lambda[1] * uvs[1].x + lambda[2] * uvs[2].x;
                    let v = lambda[0] * uvs[0].y + lambda[1] * uvs[1].y + lambda[2] * uvs[2].y;
                    sample(image, u, v)
                });
                let source = [0, 1, 2, 3].map(|i| colour[i] * texel[i] / 255.0);
                let pixel = &mut out[y as usize * size[0] + x as usize];
                let keep = 1.0 - source[3] / 255.0;
                for i in 0..4 {
                    pixel[i] = (source[i] + f32::from(pixel[i]) * keep)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
            for i in 0..3 {
                weights[i] += step_x[i];
            }
        }
        for i in 0..3 {
            row[i] += step_y[i];
        }
    }
}

/// Bilinear sample of a premultiplied image at normalised `(u, v)`, clamped
/// to the edges, as egui's renderer samples its textures.
fn sample(image: &ColorImage, u: f32, v: f32) -> [f32; 4] {
    let [width, height] = image.size;
    if width == 0 || height == 0 || image.pixels.len() < width * height {
        return [0.0; 4];
    }
    let x = u * width as f32 - 0.5;
    let y = v * height as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let clamp = |value: f32, limit: usize| (value.max(0.0) as usize).min(limit - 1);
    let (left, right) = (clamp(x0, width), clamp(x0 + 1.0, width));
    let (top, bottom) = (clamp(y0, height), clamp(y0 + 1.0, height));
    let texel =
        |x: usize, y: usize| -> [f32; 4] { image.pixels[y * width + x].to_array().map(f32::from) };
    let (a, b, c, d) = (
        texel(left, top),
        texel(right, top),
        texel(left, bottom),
        texel(right, bottom),
    );
    [0, 1, 2, 3].map(|i| {
        let upper = a[i] + (b[i] - a[i]) * fx;
        let lower = c[i] + (d[i] - c[i]) * fx;
        upper + (lower - upper) * fy
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{self, Color32, Pos2, Rect, epaint::Mesh, pos2, vec2};

    fn white() -> ColorImage {
        ColorImage::new([1, 1], vec![Color32::WHITE])
    }

    fn quad(rect: Rect, colour: Color32) -> ClippedPrimitive {
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(rect, colour);
        ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Mesh(mesh),
        }
    }

    #[test]
    fn the_font_atlas_is_copied_again_when_a_glyph_may_be_missing() {
        let glyph = |u: f32| {
            let mut mesh = Mesh::default();
            mesh.add_rect_with_uv(
                Rect::from_min_max(pos2(0., 0.), pos2(4., 4.)),
                Rect::from_min_max(pos2(u, 0.1), pos2(u + 0.01, 0.2)),
                Color32::WHITE,
            );
            ClippedPrimitive {
                clip_rect: Rect::EVERYTHING,
                primitive: Primitive::Mesh(mesh),
            }
        };
        let solid = quad(Rect::from_min_max(pos2(0., 0.), pos2(4., 4.)), Color32::RED);
        let copies = Cell::new(0);
        let copy = || {
            copies.set(copies.get() + 1);
            ColorImage::new([2, 2], vec![Color32::WHITE; 4])
        };
        let mut cache = FontCopy::default();
        cache.get(&[glyph(0.1), solid.clone()], [2, 2], 0.5, copy);
        // The same glyphs, fewer of them, or only solid shapes: no copy.
        cache.get(&[glyph(0.1)], [2, 2], 0.5, copy);
        cache.get(&[solid.clone()], [2, 2], 0.5, copy);
        assert_eq!(copies.get(), 1);
        // A glyph not seen at the copy, a larger atlas, an atlas started over.
        cache.get(&[glyph(0.3)], [2, 2], 0.5, copy);
        assert_eq!(copies.get(), 2);
        cache.get(&[glyph(0.3)], [2, 4], 0.5, copy);
        assert_eq!(copies.get(), 3);
        cache.get(&[glyph(0.3)], [2, 4], 0.1, copy);
        assert_eq!(copies.get(), 4);
        cache.get(&[glyph(0.3)], [2, 4], 0.2, copy);
        assert_eq!(copies.get(), 4);
    }

    #[test]
    fn rectangles_cover_their_pixels_once_and_blend_like_egui() {
        let font = white();
        let lookup = |id: TextureId| (id == TextureId::default()).then_some(&font);
        // Two triangles share a diagonal: a translucent quad must not show it.
        let half = Color32::from_black_alpha(128);
        let out = rasterize(
            &[quad(Rect::from_min_max(pos2(2., 2.), pos2(8., 6.)), half)],
            1.0,
            [0, 0],
            [10, 8],
            &lookup,
        );
        for y in 0..8 {
            for x in 0..10 {
                let inside = (2..8).contains(&x) && (2..6).contains(&y);
                let expected = if inside { [0, 0, 0, 128] } else { [0; 4] };
                assert_eq!(out[y * 10 + x], expected, "pixel {x},{y}");
            }
        }
        // Premultiplied "over": a red half-transparent quad over an opaque blue one.
        let out = rasterize(
            &[
                quad(
                    Rect::from_min_max(pos2(0., 0.), pos2(4., 4.)),
                    Color32::BLUE,
                ),
                quad(
                    Rect::from_min_max(pos2(0., 0.), pos2(4., 4.)),
                    Color32::from_rgba_premultiplied(100, 0, 0, 100),
                ),
            ],
            1.0,
            [0, 0],
            [4, 4],
            &lookup,
        );
        let keep: f32 = 1.0 - 100.0 / 255.0;
        assert_eq!(
            out[5],
            [
                100,
                0,
                (255.0 * keep).round() as u8,
                (100.0 + 255.0 * keep).round() as u8
            ]
        );
    }

    #[test]
    fn scale_origin_clip_and_unknown_textures_are_respected() {
        let font = white();
        let lookup = |id: TextureId| (id == TextureId::default()).then_some(&font);
        // Points at 1.5 pixels per point, a bitmap that starts at pixel (3, 3).
        let mut clipped = quad(
            Rect::from_min_max(pos2(2., 2.), pos2(6., 6.)),
            Color32::WHITE,
        );
        clipped.clip_rect = Rect::from_min_max(pos2(0., 0.), pos2(4., 100.));
        let out = rasterize(&[clipped], 1.5, [3, 3], [8, 8], &lookup);
        let covered: Vec<(usize, usize)> = (0..64)
            .filter(|i| out[*i][3] == 255)
            .map(|i| (i % 8, i / 8))
            .collect();
        // Pixels 3..6 of the screen in x (the clip ends at 6), 3..9 in y.
        assert_eq!(covered.len(), 3 * 6);
        assert!(covered.iter().all(|(x, y)| *x < 3 && *y < 6));
        let mut foreign = quad(
            Rect::from_min_max(pos2(0., 0.), pos2(4., 4.)),
            Color32::WHITE,
        );
        if let Primitive::Mesh(mesh) = &mut foreign.primitive {
            mesh.texture_id = TextureId::User(7);
        }
        assert!(
            rasterize(&[foreign], 1.0, [0, 0], [4, 4], &lookup)
                .iter()
                .all(|pixel| *pixel == [0; 4])
        );
    }

    #[test]
    fn text_tessellated_by_egui_comes_out_as_the_font_atlas_draws_it() {
        let ctx = egui::Context::default();
        let mut shapes = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200., 60.))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let painter = ui.painter();
                        painter.rect_filled(
                            Rect::from_min_size(pos2(10., 10.), vec2(120., 30.)),
                            6.,
                            Color32::from_rgb(28, 36, 49),
                        );
                        painter.text(
                            pos2(16., 14.),
                            egui::Align2::LEFT_TOP,
                            "Hint W",
                            egui::FontId::proportional(15.),
                            Color32::WHITE,
                        );
                    });
                shapes = ctx.graphics(|layers| {
                    layers
                        .get(egui::LayerId::background())
                        .map(|list| list.all_entries().cloned().collect())
                        .unwrap_or_default()
                });
            },
        );
        let primitives = ctx.tessellate(shapes, 1.0);
        let font = Arc::new(ctx.fonts(|fonts| fonts.image()));
        let lookup = |id: TextureId| (id == TextureId::default()).then_some(&*font);
        let slot = PreviewRect {
            left: 0,
            top: 0,
            right: 200,
            bottom: 60,
        };
        let at = bounds(&primitives, 1.0, slot).unwrap();
        assert!(
            at.left >= 9 && at.top >= 9 && at.right <= 131 && at.bottom <= 41,
            "{at:?}"
        );
        let out = rasterize(&primitives, 1.0, [0, 0], [200, 60], &lookup);
        // Inside the rounded box, away from its corners: the box colour or
        // text over it, always opaque.
        let opaque = (12..38)
            .flat_map(|y| (16..124).map(move |x| (x, y)))
            .all(|(x, y)| out[y * 200 + x][3] == 255);
        assert!(opaque);
        // Some pixels of the text are brighter than the box.
        let bright = (14..34)
            .flat_map(|y| (16..80).map(move |x| (x, y)))
            .filter(|(x, y)| out[y * 200 + x][0] > 200)
            .count();
        assert!(bright > 20, "{bright}");
        // Outside the box nothing is drawn.
        assert_eq!(out[5 * 200 + 5], [0; 4]);
        assert_eq!(out[50 * 200 + 150], [0; 4]);
        // The same meshes give the same fingerprint; another place another one.
        assert_eq!(
            fingerprint(&primitives, 1.0, at),
            fingerprint(&primitives, 1.0, at)
        );
        let moved = PreviewRect {
            left: at.left + 1,
            ..at
        };
        assert_ne!(
            fingerprint(&primitives, 1.0, at),
            fingerprint(&primitives, 1.0, moved)
        );
    }
}
