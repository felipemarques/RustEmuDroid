//! Capture Android's existing RGBA stream without a GPU readback.
use std::{
    fs,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tempfile::NamedTempFile;

mod replay;
use replay::Replay;

#[derive(Clone)]
pub(crate) struct Shot {
    pub width: u32,
    pub height: u32,
    pub pixels: prost::bytes::Bytes,
}

impl Shot {
    fn validate(&self) -> io::Result<()> {
        if self.width == 0
            || self.height == 0
            || u64::from(self.width)
                .checked_mul(u64::from(self.height))
                .and_then(|n| n.checked_mul(4))
                != Some(self.pixels.len() as u64)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid RGBA dimensions",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Video,
    Moment,
    Gif,
}
pub(crate) enum Event {
    Saved(PathBuf),
    Failed(String),
}

struct Stream {
    mode: Mode,
    latest: Mutex<Shot>,
    stopped: AtomicBool,
}

enum Job {
    Png,
    Recording,
    Replay,
}

type Completed = mpsc::Sender<(Job, io::Result<PathBuf>)>;

pub(crate) struct Capture {
    active: Option<Arc<Stream>>,
    replay: Option<Replay>,
    replay_saving: bool,
    tx: Completed,
    rx: mpsc::Receiver<(Job, io::Result<PathBuf>)>,
    jobs: usize,
}

impl Default for Capture {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            active: None,
            replay: None,
            replay_saving: false,
            tx,
            rx,
            jobs: 0,
        }
    }
}

impl Capture {
    pub fn mode(&self) -> Option<Mode> {
        self.active.as_ref().map(|stream| stream.mode)
    }
    pub fn busy(&self) -> bool {
        self.jobs != 0
    }
    pub fn set_replay_armed(&mut self, armed: bool) -> io::Result<()> {
        if armed && self.replay.is_none() {
            self.replay = Some(Replay::new()?);
        } else if !armed {
            self.replay = None;
        }
        Ok(())
    }
    pub fn replay_armed(&self) -> bool {
        self.replay.is_some()
    }
    pub fn replay_ready(&self) -> bool {
        self.replay
            .as_ref()
            .is_some_and(|replay| replay.ready(Instant::now()))
    }
    pub fn replay_saving(&self) -> bool {
        self.replay_saving
    }
    pub fn reset_replay(&mut self) {
        if let Some(replay) = &mut self.replay {
            replay.reset();
        }
    }
    pub fn update_replay(&mut self, shot: &Shot) {
        if let Some(replay) = &mut self.replay {
            replay.observe(shot, Instant::now());
        }
    }
    pub fn save_replay(&mut self, dir: &Path) -> io::Result<()> {
        if self.busy() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let replay = self.replay.as_ref().ok_or(io::ErrorKind::NotFound)?;
        replay.save(Instant::now(), dir, self.tx.clone())?;
        self.replay_saving = true;
        self.jobs += 1;
        Ok(())
    }
    pub fn update(&self, shot: &Shot) {
        if let Some(stream) = &self.active {
            let mut latest = stream.latest.lock().unwrap();
            if (shot.width, shot.height) == (latest.width, latest.height) {
                *latest = shot.clone();
            } else {
                // Finish the playable clip at the orientation boundary.
                stream.stopped.store(true, Ordering::Release);
            }
        }
    }
    pub fn stop(&self) {
        if let Some(stream) = &self.active {
            stream.stopped.store(true, Ordering::Release);
        }
    }
    pub fn png(&mut self, shot: &Shot, dir: &Path) -> io::Result<()> {
        shot.validate()?;
        let shot = shot.clone();
        self.export_png(dir, move |dir| save_png(&shot, dir))
    }
    #[cfg(windows)]
    pub fn png_from_guest(
        &mut self,
        dir: &Path,
        read: impl FnOnce() -> io::Result<Vec<u8>> + Send + 'static,
    ) -> io::Result<()> {
        self.export_png(dir, move |dir| save_guest_png(&read()?, dir))
    }
    fn export_png(
        &mut self,
        dir: &Path,
        save: impl FnOnce(&Path) -> io::Result<PathBuf> + Send + 'static,
    ) -> io::Result<()> {
        if self.jobs >= 3 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let (dir, tx) = (dir.to_owned(), self.tx.clone());
        thread::Builder::new()
            .name("capture-png".into())
            .spawn(move || {
                let _ = tx.send((Job::Png, save(&dir)));
            })?;
        self.jobs += 1;
        Ok(())
    }
    pub fn start(&mut self, mode: Mode, shot: &Shot, dir: &Path) -> io::Result<()> {
        shot.validate()?;
        if self.active.is_some() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let stream = Arc::new(Stream {
            mode,
            latest: Mutex::new(shot.clone()),
            stopped: false.into(),
        });
        let (worker, dir, tx) = (stream.clone(), dir.to_owned(), self.tx.clone());
        thread::Builder::new()
            .name("capture-encoder".into())
            .spawn(move || {
                let _ = tx.send((Job::Recording, record(&worker, &dir)));
            })?;
        self.active = Some(stream);
        self.jobs += 1;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Event> {
        let (job, result) = self.rx.try_recv().ok()?;
        self.jobs -= 1;
        match job {
            Job::Png => {}
            Job::Recording => self.active = None,
            Job::Replay => self.replay_saving = false,
        }
        Some(match result {
            Ok(path) => Event::Saved(path),
            Err(e) => Event::Failed(e.to_string()),
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn output(dir: &Path, ext: &str) -> io::Result<(NamedTempFile, PathBuf)> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(dir)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!(
        "RustEmuDroid-{stamp}-{}-{sequence}.{ext}",
        std::process::id()
    ));
    Ok((NamedTempFile::new_in(dir)?, path))
}

fn save_png(shot: &Shot, dir: &Path) -> io::Result<PathBuf> {
    let (mut temp, path) = output(dir, "png")?;
    let mut encoder = png::Encoder::new(temp.as_file_mut(), shot.width, shot.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&shot.pixels)?;
    writer.finish()?;
    temp.persist_noclobber(&path)?;
    Ok(path)
}

#[cfg(windows)]
fn save_guest_png(bytes: &[u8], dir: &Path) -> io::Result<PathBuf> {
    const MAX_PIXELS_BYTES: usize = 48 * 1024 * 1024;
    let decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(bytes),
        png::Limits {
            bytes: MAX_PIXELS_BYTES,
        },
    );
    let mut reader = decoder.read_info()?;
    if reader.info().animation_control.is_some()
        || reader
            .output_buffer_size()
            .is_none_or(|n| n > MAX_PIXELS_BYTES)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid Android screenshot size or format",
        ));
    }
    // Validate the complete frame before committing Android's original PNG bytes.
    while reader.next_row()?.is_some() {}
    reader.finish()?;
    let (mut temp, path) = output(dir, "png")?;
    temp.write_all(bytes)?;
    temp.persist_noclobber(&path)?;
    Ok(path)
}

fn record(stream: &Stream, dir: &Path) -> io::Result<PathBuf> {
    let initial = stream.latest.lock().unwrap().clone();
    encode(
        stream.mode,
        (initial.width, initial.height),
        "rgba",
        dir,
        |pipe, fps| {
            let period = Duration::from_secs_f64(1.0 / f64::from(fps));
            let mut tick = Instant::now();
            let mut frame_count = 0;
            loop {
                let shot = stream.latest.lock().unwrap().clone();
                pipe.write_all(&shot.pixels)?;
                frame_count += 1;
                if stream.stopped.load(Ordering::Acquire)
                    || (stream.mode != Mode::Video && frame_count >= 3 * fps)
                {
                    return Ok(());
                }
                tick += period;
                thread::sleep(tick.saturating_duration_since(Instant::now()));
                if stream.mode == Mode::Video {
                    tick = tick.max(Instant::now());
                }
            }
        },
    )
}

fn encode(
    mode: Mode,
    (width, height): (u32, u32),
    pixel_format: &str,
    dir: &Path,
    write: impl FnOnce(&mut std::process::ChildStdin, u32) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let (ext, fps) = match mode {
        Mode::Video => ("mp4", 30u32),
        Mode::Moment => ("jxl", replay::FPS),
        Mode::Gif => ("gif", 10),
    };
    let (temp, path) = output(dir, ext)?;
    let mut log = tempfile::tempfile()?;
    let mut cmd =
        Command::new(std::env::var_os("RUSTEMUDROID_FFMPEG").unwrap_or_else(|| "ffmpeg".into()));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "rawvideo",
        "-pixel_format",
        pixel_format,
        "-video_size",
    ])
    .arg(format!("{width}x{height}"))
    .arg("-framerate")
    .arg(fps.to_string());
    if mode == Mode::Video {
        cmd.args(["-use_wallclock_as_timestamps", "1"]);
    }
    cmd.args([
        "-i",
        "pipe:0",
        "-an",
        "-filter_threads",
        "1",
        "-threads",
        "2",
    ]);
    match mode {
        Mode::Video => {
            cmd.args([
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "22",
                "-pix_fmt",
                "yuv420p",
                "-fps_mode",
                "vfr",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
            ]);
        }
        Mode::Moment | Mode::Gif => {
            if pixel_format == "rgba" {
                cmd.args([
                    "-vf",
                    "scale=w='min(720,iw)':h='min(720,ih)':force_original_aspect_ratio=decrease",
                ]);
            }
            if mode == Mode::Moment {
                cmd.args([
                    "-c:v",
                    "libjxl_anim",
                    "-effort",
                    "2",
                    "-distance",
                    "1.0",
                    "-pix_fmt",
                    "rgb24",
                    "-f",
                    "image2pipe",
                ]);
            } else {
                cmd.args(["-loop", "0", "-f", "gif"]);
            }
        }
    }
    let mut child = cmd
        .arg(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(log.try_clone()?)
        .spawn()
        .map_err(|err| io::Error::new(err.kind(), format!("FFmpeg: {err}")))?;
    let mut pipe = child.stdin.take().expect("configured piped input");
    let write_result = write(&mut pipe, fps);
    drop(pipe);
    if !child.wait()?.success() {
        log.rewind()?;
        let mut errors = String::new();
        log.take(4096).read_to_string(&mut errors)?;
        return Err(io::Error::other(format!(
            "FFmpeg ({ext}): {}",
            errors.trim()
        )));
    }
    write_result?;
    if temp.as_file().metadata()?.len() == 0 {
        return Err(io::Error::other("FFmpeg: empty output"));
    }
    temp.persist_noclobber(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests;
