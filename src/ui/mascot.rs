use std::{
    f64::consts::TAU,
    sync::{Arc, OnceLock},
    time::Duration,
};

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Vec2, pos2, vec2};

const SEGMENTS: u32 = 24;
const CYCLE: u64 = 60;
const FIRST_MOVE: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    left_claw: f32,
    right_claw: f32,
    step: f32,
    blink: f32,
}

impl Pose {
    const REST: Self = Self {
        left_claw: 0.,
        right_claw: 0.,
        step: 0.,
        blink: 0.,
    };
}

fn pose_at(frame: u64) -> (Pose, u64) {
    let frame_in_cycle = frame % CYCLE;
    let (pose, next) = match frame_in_cycle {
        0..FIRST_MOVE => (Pose::REST, FIRST_MOVE),
        FIRST_MOVE..=16 => {
            let motion = (frame_in_cycle - FIRST_MOVE) as f64 / 12.;
            let weight = (motion * std::f64::consts::PI).sin();
            let wave = (motion * TAU).sin() * weight;
            (
                Pose {
                    left_claw: (wave * -0.115) as f32,
                    right_claw: (wave * -0.095) as f32,
                    step: wave as f32,
                    blink: 0.,
                },
                frame_in_cycle + 1,
            )
        }
        17..=32 => (Pose::REST, 33),
        33..=35 => (
            Pose {
                blink: if frame_in_cycle == 34 { 0.88 } else { 0.49 },
                ..Pose::REST
            },
            frame_in_cycle + 1,
        ),
        _ => (Pose::REST, CYCLE + FIRST_MOVE),
    };
    (pose, frame - frame_in_cycle + next)
}

#[derive(Clone)]
struct FrameCache {
    epoch: f64,
    last_seen: f64,
    mesh: Option<(Pose, Rect, egui::TextureId, Arc<egui::Mesh>)>,
}

pub(super) const fn enabled_by_default() -> bool {
    true
}

pub(super) fn show(
    ui: &mut egui::Ui,
    brand: egui::TextureId,
    size: f32,
    enabled: bool,
    over_game: bool,
) {
    let minimized = ui.input(|input| input.viewport().minimized.unwrap_or(false));
    draw(
        ui,
        brand,
        size,
        enabled && !minimized && system_allows_motion(ui.ctx()),
        over_game,
    );
}

fn draw(ui: &mut egui::Ui, brand: egui::TextureId, size: f32, animated: bool, over_game: bool) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let timer = response.id.with("mascot");
    if !ui.is_rect_visible(rect) {
        return;
    }
    let art = rect.shrink(size * 0.03);
    if !animated {
        ui.ctx().data_mut(|data| data.remove::<FrameCache>(timer));
        ui.painter().image(
            brand,
            art,
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            Color32::WHITE,
        );
        return;
    }
    let now = ui.input(|input| input.time);
    let interval = if over_game { 1. / 8. } else { 1. / 12. };
    let mut cache = ui
        .ctx()
        .data(|data| data.get_temp::<FrameCache>(timer))
        .filter(|cache| now - cache.last_seen < 4.)
        .unwrap_or(FrameCache {
            epoch: now,
            last_seen: now,
            mesh: None,
        });
    let elapsed = (now - cache.epoch).max(0.);
    let frame = (elapsed / interval).floor() as u64;
    let (pose, next) = pose_at(frame);
    if pose == Pose::REST {
        ui.painter().image(
            brand,
            art,
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            Color32::WHITE,
        );
    } else {
        let needs_mesh = cache
            .mesh
            .as_ref()
            .is_none_or(|(cached, rect, texture, _)| {
                *cached != pose || *rect != art || *texture != brand
            });
        if needs_mesh {
            cache.mesh = Some((pose, art, brand, Arc::new(mesh(art, brand, pose))));
        }
        ui.painter()
            .add(Shape::Mesh(cache.mesh.as_ref().unwrap().3.clone()));
    }
    cache.last_seen = now;
    ui.ctx().data_mut(|data| data.insert_temp(timer, cache));
    ui.ctx().request_repaint_after(Duration::from_secs_f64(
        (next as f64 * interval - elapsed).max(0.001),
    ));
}

fn mesh(art: Rect, texture: egui::TextureId, pose: Pose) -> egui::Mesh {
    let dimension = SEGMENTS + 1;
    let mut mesh = egui::Mesh::with_texture(texture);
    for row in 0..dimension {
        for col in 0..dimension {
            let uv = pos2(col as f32 / SEGMENTS as f32, row as f32 / SEGMENTS as f32);
            let point = deform(uv, pose);
            mesh.vertices.push(egui::epaint::Vertex {
                pos: art.min + point.to_vec2() * art.size(),
                uv,
                color: Color32::WHITE,
            });
        }
    }
    for row in 0..SEGMENTS {
        for col in 0..SEGMENTS {
            let index = row * dimension + col;
            mesh.add_triangle(index, index + 1, index + dimension + 1);
            mesh.add_triangle(index, index + dimension + 1, index + dimension);
        }
    }
    mesh
}

fn smooth(start: f32, end: f32, value: f32) -> f32 {
    let x = ((value - start) / (end - start)).clamp(0., 1.);
    x * x * (3. - 2. * x)
}

fn deform(uv: Pos2, pose: Pose) -> Pos2 {
    // Both claws and the visor stay on the same continuous bitmap mesh.
    let claw_y = 1. - smooth(0.49, 0.63, uv.y);
    let left = (1. - smooth(0.25, 0.37, uv.x)) * claw_y;
    let right = smooth(0.63, 0.75, uv.x) * claw_y;
    let pivot = if uv.x < 0.5 {
        pos2(0.23, 0.53)
    } else {
        pos2(0.77, 0.53)
    };
    let angle = pose.left_claw * left + pose.right_claw * right;
    let (sin, cos) = angle.sin_cos();
    let offset = uv - pivot;
    let mut point = pivot
        + vec2(
            offset.x * cos - offset.y * sin,
            offset.x * sin + offset.y * cos,
        );
    let visor_x = smooth(0.31, 0.355, uv.x) * (1. - smooth(0.65, 0.69, uv.x));
    let visor_y = uv.y - 0.457;
    let blink = pose.blink * visor_x * (1. - smooth(0.036, 0.084, visor_y.abs()));
    point.y -= visor_y * blink;
    point + vec2(pose.step * 0.012, -pose.step.abs() * 0.017)
}

#[cfg(windows)]
fn system_allows_motion(_: &egui::Context) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };

    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let mut enabled = 1i32;
        // SPI_GETCLIENTAREAANIMATION takes a pointer to a Win32 BOOL.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            )
        };
        ok == 0 || enabled != 0
    })
}

#[cfg(target_os = "linux")]
fn system_allows_motion(ctx: &egui::Context) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ENABLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    let enabled = ENABLED.get_or_init(|| {
        // Keep the logo still until the desktop settings query completes.
        let enabled = Arc::new(AtomicBool::new(false));
        let worker = enabled.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("desktop-motion".into())
            .spawn(move || {
                worker.store(linux_allows_motion(), Ordering::Release);
                ctx.request_repaint();
            });
        enabled
    });
    enabled.load(Ordering::Acquire)
}

#[cfg(target_os = "linux")]
fn linux_allows_motion() -> bool {
    use std::process::Command;

    let portal = Command::new("gdbus")
        .args([
            "call",
            "--timeout",
            "2",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.Read",
            "org.freedesktop.appearance",
            "reduced-motion",
        ])
        .output();
    if let Ok(portal) = portal
        && portal.status.success()
    {
        let value = String::from_utf8_lossy(&portal.stdout);
        if let Some((_, value)) = value.split_once("uint32 ") {
            return value.split(|c: char| !c.is_ascii_digit()).next() != Some("1");
        }
    }
    let gnome = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .output();
    !gnome.is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"false")
}

#[cfg(not(any(windows, target_os = "linux")))]
fn system_allows_motion(_: &egui::Context) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poses_include_rest_claw_motion_and_a_brief_blink() {
        assert_eq!(pose_at(1), (Pose::REST, FIRST_MOVE));
        let (wave, _) = pose_at(8);
        assert!(wave.left_claw.abs() > 0.04);
        assert!(wave.right_claw.abs() > 0.04);
        assert_eq!(wave.blink, 0.);
        assert_eq!(pose_at(20), (Pose::REST, 33));
        assert!(pose_at(34).0.blink > 0.8);
        assert_eq!(pose_at(48), (Pose::REST, CYCLE + FIRST_MOVE));
    }

    #[test]
    fn all_animated_triangles_stay_ordered_inside_the_ui_box() {
        let box_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(120.));
        let art = box_rect.shrink(3.6);
        let export = std::env::var_os("RUSTEMUDROID_MASCOT_GEOMETRY");
        let mut frames = Vec::new();
        for frame in 0..CYCLE {
            let mesh = mesh(art, egui::TextureId::Managed(0), pose_at(frame).0);
            assert!(mesh.is_valid());
            for triangle in mesh.indices.chunks_exact(3) {
                let triangle: Vec<_> = triangle
                    .iter()
                    .map(|index| mesh.vertices[*index as usize].pos)
                    .collect();
                let (a, b) = (triangle[1] - triangle[0], triangle[2] - triangle[0]);
                assert!(a.x * b.y - a.y * b.x > 0.);
            }
            // The alpha-bearing part of the bitmap stays inside the allocated box.
            for vertex in mesh
                .vertices
                .iter()
                .filter(|v| (0.025..=0.975).contains(&v.uv.x) && (0.09..=0.91).contains(&v.uv.y))
            {
                assert!(box_rect.contains(vertex.pos), "frame {frame}, {vertex:?}");
            }
            if export.is_some() {
                frames.push(serde_json::json!({
                    "points": mesh.vertices.iter().map(|vertex|
                        [vertex.pos.x, vertex.pos.y, vertex.uv.x, vertex.uv.y]
                    ).collect::<Vec<_>>(),
                    "indices": if frame == 0 { mesh.indices } else { Vec::new() },
                }));
            }
        }
        if let Some(path) = export {
            let file = std::fs::File::create(path).unwrap();
            serde_json::to_writer(file, &frames).unwrap();
        }
    }

    #[test]
    fn disabled_motion_has_no_timer_after_initial_layout() {
        let ctx = egui::Context::default();
        for frame in 0..4 {
            let out = ctx.run(
                egui::RawInput {
                    time: Some(frame as f64 / 12.),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw(ui, egui::TextureId::Managed(0), 120., false, false)
                    });
                },
            );
            if frame == 3 {
                assert_eq!(
                    out.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
                    Duration::MAX
                );
            }
        }
    }
}
