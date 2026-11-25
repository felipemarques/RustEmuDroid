use super::*;
use std::{collections::HashSet, hint::black_box, io::BufReader};

fn shot(width: u32, height: u32, pixels: Vec<u8>) -> Shot {
    Shot {
        width,
        height,
        pixels: pixels.into(),
    }
}

fn await_capture(capture: &mut Capture) -> Option<PathBuf> {
    match capture.poll()? {
        Event::Saved(path) => Some(path),
        Event::Failed(error) => panic!("{error}"),
    }
}

#[test]
fn png_recovers_native_dimensions_and_rgba_pixels() {
    let directory = tempfile::tempdir().unwrap();
    let input = shot(
        2,
        2,
        vec![
            9, 20, 33, 44, 66, 55, 44, 33, 87, 98, 109, 120, 241, 242, 243, 255,
        ],
    );
    let mut capture = Capture::default();
    assert!(
        capture
            .png(&shot(2, 2, vec![0; 15]), directory.path())
            .is_err()
    );
    capture.png(&input, directory.path()).unwrap();
    assert!(capture.busy());
    let deadline = Instant::now() + Duration::from_secs(5);
    let path = loop {
        if let Some(path) = await_capture(&mut capture) {
            break path;
        }
        assert!(Instant::now() < deadline, "PNG worker timed out");
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(path.extension().unwrap(), "png");
    assert!(!capture.busy());
    let mut reader = png::Decoder::new(BufReader::new(fs::File::open(path).unwrap()))
        .read_info()
        .unwrap();
    let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut rgba).unwrap();
    assert_eq!((info.width, info.height), (2, 2));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(&rgba[..info.buffer_size()], input.pixels.as_ref());
}

#[cfg(windows)]
#[test]
fn guest_png_exports_are_deferred_preserve_pixels_and_use_unique_capture_names() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let png = fs::read(
        save_png(
            &shot(2, 1, vec![17, 81, 209, 255, 211, 7, 44, 255]),
            fixture.path(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut capture = Capture::default();
    let (release, wait) = mpsc::channel();
    let first = png.clone();
    capture
        .png_from_guest(dir.path(), move || {
            wait.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok(first)
        })
        .unwrap();
    assert!(capture.busy());
    assert!(capture.poll().is_none());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    capture
        .png_from_guest(dir.path(), {
            let png = png.clone();
            move || Ok(png)
        })
        .unwrap();
    release.send(()).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saved = HashSet::new();
    while capture.busy() {
        if let Some(path) = await_capture(&mut capture) {
            assert!(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("RustEmuDroid-")
            );
            assert_eq!(path.parent(), Some(dir.path()));
            assert_eq!(path.extension().unwrap(), "png");
            assert_eq!(fs::read(&path).unwrap(), png);
            assert!(saved.insert(path));
        }
        assert!(Instant::now() < deadline, "PNG exports did not finish");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(saved.len(), 2);
    assert!(save_guest_png(&png[..png.len() - 8], dir.path()).is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn a_size_change_stops_video_without_replacing_the_last_frame() {
    let first = shot(2, 3, vec![0; 24]);
    let second = shot(2, 3, vec![127; 24]);
    for changed in [shot(3, 2, vec![255; 24]), shot(4, 6, vec![255; 96])] {
        let stream = Arc::new(Stream {
            mode: Mode::Video,
            latest: Mutex::new(first.clone()),
            stopped: false.into(),
        });
        let mut capture = Capture::default();
        capture.active = Some(stream.clone());
        capture.update(&second);
        assert!(!stream.stopped.load(Ordering::Acquire));
        capture.update(&changed);
        assert!(stream.stopped.load(Ordering::Acquire));
        let last = stream.latest.lock().unwrap();
        assert_eq!((last.width, last.height), (2, 3));
        assert_eq!(last.pixels, second.pixels);
    }
}

/// Capture motion on a 1920×1080 canvas with the desktop API and decode each output.
/// Run with `cargo test --locked --release --features ui synthetic_scene -- --ignored --nocapture`.
#[test]
#[ignore = "requires ffprobe and FFmpeg with libx264 and libjxl_anim encoders and decoders"]
fn synthetic_scene_encodes_decodable_motion_and_duration() {
    let directory = tempfile::tempdir().unwrap();
    let samples: Vec<_> = (0..12).map(moving_scene).collect();
    let idle = Capture::default();
    let start = Instant::now();
    for index in 0..200_000 {
        black_box(&idle).update(black_box(&samples[index % samples.len()]));
    }
    eprintln!(
        "Idle API average {:.2} ns per update",
        start.elapsed().as_secs_f64() * 1e9 / 200_000.
    );

    for (mode, extension, dimensions, codec, minimum_frames) in [
        (Mode::Video, "mp4", (1920, 1080), "h264", 30),
        (Mode::Moment, "jxl", (720, 405), "jpegxl_anim", 30),
        (Mode::Gif, "gif", (720, 405), "gif", 30),
    ] {
        let mut capture = Capture::default();
        capture.start(mode, &samples[0], directory.path()).unwrap();
        let start = Instant::now();
        let frame_period = Duration::from_secs_f64(1. / 60.);
        let mut next_frame = start;
        let mut cost = vec![];
        let file = loop {
            if let Some(file) = await_capture(&mut capture) {
                break file;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "{mode:?} timed out"
            );
            // Shift the scene at 12 steps/s so even the 10 FPS encoders sample motion.
            let frame = &samples[(start.elapsed().as_millis() / 83) as usize % samples.len()];
            let update = Instant::now();
            capture.update(black_box(frame));
            cost.push(update.elapsed().as_nanos());
            if mode == Mode::Video && start.elapsed() >= Duration::from_secs(3) {
                capture.stop();
            }
            next_frame += frame_period;
            thread::sleep(next_frame.saturating_duration_since(Instant::now()));
        };
        assert!(!capture.busy());
        assert_eq!(file.extension().unwrap(), extension);
        assert!(cost.len() >= 120);
        cost.sort_unstable();
        let probe = ffprobe(&file);
        let streams = probe["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 1, "{mode:?} should have no audio stream");
        let stream = &streams[0];
        assert_eq!(stream["codec_name"], codec);
        assert_eq!(stream["width"], dimensions.0);
        assert_eq!(stream["height"], dimensions.1);

        let (duration, count, distinct) = decoded_motion(&file);
        assert!(count >= minimum_frames, "{mode:?} has only {count} frames");
        if mode != Mode::Video {
            assert_eq!(count, 30, "{mode:?} should keep 30 frames");
        }
        assert!(
            distinct >= 6,
            "{mode:?} has only {distinct} distinct frames"
        );
        assert!(
            (2.7..=3.5).contains(&duration),
            "{mode:?} duration {duration:.3}s"
        );
        eprintln!(
            "{mode:?} {extension}: {} bytes, {count} decoded frames ({distinct} distinct), {duration:.3}s, {:.2} encoded FPS, desktop update median/P95 {}/{} ns",
            file.metadata().unwrap().len(),
            count as f64 / duration,
            cost[cost.len() / 2],
            cost[(cost.len() * 95).div_ceil(100) - 1],
        );
    }
}

fn moving_scene(index: u32) -> Shot {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    let left = 110 * index;
    let top = 130 + (index % 5) * 115;
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let foreground = (left..left + 400).contains(&x) && (top..top + 320).contains(&y);
            let tint = ((x / 80 + y / 60) % 4) as u8 * 8;
            let rgb = if foreground {
                [240, 72 + tint, 22, 255]
            } else {
                [36 + tint, 79, 110 + tint, 255]
            };
            pixels.extend_from_slice(&rgb);
        }
    }
    shot(WIDTH, HEIGHT, pixels)
}

fn ffprobe(path: &Path) -> serde_json::Value {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(path)
        .output()
        .expect("ffprobe is required");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

pub(super) fn decoded_motion(path: &Path) -> (f64, usize, usize) {
    let ffmpeg = std::env::var_os("RUSTEMUDROID_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let output = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "framemd5",
            "pipe:1",
        ])
        .output()
        .expect("FFmpeg is required");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::str::from_utf8(&output.stdout).unwrap();
    let base = text
        .lines()
        .find_map(|line| line.strip_prefix("#tb 0: "))
        .unwrap();
    let (numerator, denominator) = base.split_once('/').unwrap();
    let time_base = numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap();
    let frames: Vec<_> = text
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let parts: Vec<_> = line.split(',').map(str::trim).collect();
            (
                parts[2].parse::<i64>().unwrap(),
                parts[3].parse::<i64>().unwrap(),
                parts[5],
            )
        })
        .collect();
    let (first_pts, _, _) = frames.first().unwrap();
    let (last_pts, last_duration, _) = frames.last().unwrap();
    let time = (last_pts + last_duration - first_pts) as f64 * time_base;
    let unique: HashSet<_> = frames.iter().map(|(_, _, hash)| *hash).collect();
    (time, frames.len(), unique.len())
}
