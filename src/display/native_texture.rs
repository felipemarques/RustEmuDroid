use crate::capture::Shot;
use eframe::{
    egui,
    egui_glow::{CallbackFn, Painter},
    glow::{self, HasContext},
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Uploaded {
    size: [u32; 2],
    serial: u64,
    #[cfg(all(test, windows))]
    count: usize,
}

#[derive(Default)]
pub(super) struct NativeTexture {
    serial: u64,
    uploaded: Arc<Mutex<Uploaded>>,
}

impl NativeTexture {
    pub(super) fn advance(&mut self) {
        self.serial += 1;
    }

    #[cfg(all(test, windows))]
    pub(super) fn uploaded_count(&self) -> usize {
        self.uploaded.lock().unwrap().count
    }

    // The managed 1x1 texture owns the native GL allocation. Egui applies
    // texture deltas before painting callbacks and frees the object on drop.
    pub(super) fn queue(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        id: egui::TextureId,
        shot: &Shot,
    ) {
        let serial = self.serial;
        if self.uploaded.lock().unwrap().serial == serial {
            return;
        }
        let uploaded = self.uploaded.clone();
        let shot = shot.clone();
        painter.add(egui::PaintCallback {
            rect,
            callback: Arc::new(CallbackFn::new(move |_, painter| {
                let mut uploaded = uploaded.lock().unwrap();
                if serial <= uploaded.serial {
                    return;
                }
                if upload(painter, id, &shot, uploaded.size) {
                    uploaded.size = [shot.width, shot.height];
                    uploaded.serial = serial;
                    #[cfg(all(test, windows))]
                    {
                        uploaded.count += 1;
                    }
                }
            })),
        });
    }
}

pub(super) fn supported(gl: Option<&glow::Context>) -> bool {
    supported_version(gl.map(|gl| gl.version().major))
}

fn supported_version(major: Option<u32>) -> bool {
    // Both the desktop and ES 3.0 APIs provide RGBA8, PBOs and row/skip unpack state.
    major.is_some_and(|major| major >= 3)
}

fn upload(painter: &Painter, id: egui::TextureId, shot: &Shot, previous: [u32; 2]) -> bool {
    let Some(texture) = painter.texture(id) else {
        return false;
    };
    let Ok(width) = i32::try_from(shot.width) else {
        return false;
    };
    let Ok(height) = i32::try_from(shot.height) else {
        return false;
    };

    // Matches Painter's native RGBA8/UNSIGNED_BYTE path. Keep all unpack
    // state local to this callback; PBO-bound GL interprets a pointer as an offset.
    let gl = painter.gl();
    let data = glow::PixelUnpackData::Slice(Some(&shot.pixels));
    unsafe {
        let previous_texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
        let previous_pbo = gl.get_parameter_buffer(glow::PIXEL_UNPACK_BUFFER_BINDING);
        if previous_pbo.is_some() {
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        }
        let unpack = [
            (glow::UNPACK_ALIGNMENT, 1),
            (glow::UNPACK_ROW_LENGTH, 0),
            (glow::UNPACK_SKIP_ROWS, 0),
            (glow::UNPACK_SKIP_PIXELS, 0),
        ]
        .map(|(key, desired)| {
            let previous = gl.get_parameter_i32(key);
            if previous != desired {
                gl.pixel_store_i32(key, desired);
            }
            (key, previous, desired)
        });
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        if previous == [shot.width, shot.height] {
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                width,
                height,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                data,
            );
        } else {
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                data,
            );
        }
        gl.bind_texture(glow::TEXTURE_2D, previous_texture);
        for (key, previous, desired) in unpack {
            if previous != desired {
                gl.pixel_store_i32(key, previous);
            }
        }
        if previous_pbo.is_some() {
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, previous_pbo);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_without_glow_or_with_pre_gl3_contexts() {
        assert!(!supported(None));
        assert!(!supported_version(Some(2)));
        assert!(supported_version(Some(3)));
        assert!(supported_version(Some(4)));
    }
}
