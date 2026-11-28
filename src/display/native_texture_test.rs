use super::{engine_rpc::Rotation, *};
use egui::{Color32, ViewportCommand};
use std::{cell::Cell, rc::Rc};
use winit::platform::windows::EventLoopBuilderExtWindows;

const OFFSET: f32 = 128.0;

fn frame(stage: usize) -> Frame {
    let (width, height) = if stage < 2 { (3, 2) } else { (2, 3) };
    let mut bytes = vec![0x99];
    for index in 0..6 {
        let alpha = 255 - index * 17;
        bytes.extend([index * 21, 132 - index * 17, 20 + index * 26, alpha]);
    }
    if stage == 1 {
        bytes[1..5].copy_from_slice(&[231, 107, 11, 255]);
    }
    Frame {
        format: Some(Format {
            width,
            height,
            rotation: Some(Rotation {
                value: i32::from(stage >= 2),
            }),
            ..Default::default()
        }),
        pixels: prost::bytes::Bytes::from(bytes).slice(1..),
    }
}

struct Fixture {
    baseline: Display,
    direct: Option<Display>,
    direct_id: Option<egui::TextureId>,
    stage: usize,
    waiting: bool,
    drop_checked: Arc<Mutex<bool>>,
    done: Rc<Cell<bool>>,
    deadline: Instant,
}

impl eframe::App for Fixture {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        assert!(Instant::now() < self.deadline, "native fixture timed out");
        if self.stage == 6 {
            if *self.drop_checked.lock().unwrap() {
                self.done.set(true);
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }
        if self.stage == 5 {
            let checked = self.drop_checked.clone();
            let id = self.direct_id.unwrap();
            ctx.layer_painter(egui::LayerId::background())
                .add(egui::PaintCallback {
                    rect: ctx.content_rect(),
                    callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_, painter| {
                        assert!(
                            painter.texture(id).is_none(),
                            "managed texture was not freed"
                        );
                        *checked.lock().unwrap() = true;
                    })),
                });
            self.stage = 6;
            ctx.request_repaint();
            return;
        }
        if let Some(event) = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        }) {
            let ppp = ctx.pixels_per_point();
            let stride = (ppp * OFFSET).round() as usize;
            let sample = |x: usize, y: usize| event.pixels[y * event.width() + x];
            let mut distinct = std::collections::HashSet::new();
            for y in 12..(ppp * 92.).floor() as usize {
                for x in 12..(ppp * 92.).floor() as usize {
                    let left = sample(x, y);
                    let right = sample(x + stride, y);
                    distinct.insert(left);
                    // Fractional sampling can round by one LSB at a different
                    // horizontal origin even for identical texels.
                    for (expected, observed) in left.to_array().into_iter().zip(right.to_array()) {
                        assert!(
                            expected.abs_diff(observed) <= 1,
                            "pixel ({x}, {y}): {left:?} vs {right:?} at stage {}",
                            self.stage
                        );
                    }
                }
            }
            assert!(
                distinct.len() > 24,
                "the frame and overlay must actually render"
            );
            let direct = self.direct.as_ref().unwrap();
            assert_eq!(
                direct.native_texture.as_ref().unwrap().uploaded_count(),
                self.stage.min(2) + 1
            );
            self.stage += 1;
            self.waiting = false;
            if self.stage == 4 {
                self.direct = None;
                ctx.request_repaint();
                self.stage = 5;
                return;
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(32, 45, 77)))
            .show(ctx, |ui| {
                let direct = self.direct.as_mut().unwrap();
                if !self.waiting && self.stage < 3 {
                    let frame = frame(self.stage);
                    for display in [&mut self.baseline, &mut *direct] {
                        display.state.lock().unwrap().latest = Some(frame.clone());
                    }
                }
                for (slot, display) in [&mut self.baseline, &mut *direct].into_iter().enumerate() {
                    let offset = (slot as f32) * OFFSET;
                    let rect = Rect::from_min_size(egui::pos2(0. + offset, 0.), Vec2::splat(100.));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        ui.set_clip_rect(rect);
                        display.show(ui, false, Locale::default());
                        let overlay =
                            Rect::from_min_size(egui::pos2(offset + 36., 46.), egui::vec2(24., 8.));
                        ui.painter().rect_filled(overlay, 0.0, Color32::YELLOW);
                    });
                }
                self.direct_id = direct.texture.as_ref().map(TextureHandle::id);
                let capture = direct.shot().unwrap();
                let reference = self.baseline.shot().unwrap();
                assert_eq!(capture.pixels, reference.pixels);
                assert_eq!(
                    (capture.width, capture.height),
                    (reference.width, reference.height)
                );
                assert_eq!(capture.pixels, frame(self.stage.min(2)).pixels);
            });
        if !self.waiting {
            self.waiting = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

#[test]
#[ignore = "opens an isolated Windows OpenGL fixture"]
fn native_glow_matches_managed_texture() {
    let done = Rc::new(Cell::new(false));
    let finished = done.clone();
    eframe::run_native(
        "RustEmuDroid native texture fixture",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport: egui::ViewportBuilder::default().with_inner_size([272., 112.]),
            event_loop_builder: Some(Box::new(|builder| {
                builder.with_any_thread(true);
            })),
            ..Default::default()
        },
        Box::new(move |cc| {
            use eframe::glow::HasContext;
            let gl = cc.gl.as_ref().unwrap();
            eprintln!("Fixture renderer: {}", unsafe {
                gl.get_parameter_string(eframe::glow::RENDERER)
            });
            let new = || Display::new(Default::default(), mpsc::unbounded_channel().0);
            let mut baseline = new();
            baseline.enable_native_texture(None);
            assert!(baseline.native_texture.is_none());
            let mut direct = new();
            direct.enable_native_texture(cc.gl.as_deref());
            assert!(direct.native_texture.is_some());
            Ok(Box::new(Fixture {
                baseline,
                direct: Some(direct),
                direct_id: None,
                stage: 0,
                waiting: false,
                drop_checked: Default::default(),
                done: finished,
                deadline: Instant::now() + Duration::from_secs(12),
            }))
        }),
    )
    .unwrap();
    assert!(done.get());
}
