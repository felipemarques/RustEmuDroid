//! The same paced, single-slot producer is shared by both viewer executables.

use crate::source::{Pattern, read_rgba, rgba_len};
use std::{
    fs::File,
    io::{self, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub seconds: u32,
    pub warmup: u32,
    pub raw: Option<PathBuf>,
}

pub struct Frame {
    pub sequence: u64,
    pub produced_at: Instant,
    pub measured: bool,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct Status {
    pub started: Option<Instant>,
    pub ended: Option<Instant>,
    pub produced: u64,
    pub replaced: u64,
    pub late_slots: u64,
    pub duplicates: u64,
    pub error: Option<String>,
}

#[derive(Default)]
struct Slot {
    status: Status,
    latest: Option<Frame>,
}

pub struct Feed(Arc<Mutex<Slot>>);

impl Feed {
    pub fn take(&self) -> Option<Frame> {
        self.0.lock().unwrap().latest.take()
    }

    pub fn status(&self) -> Status {
        self.0.lock().unwrap().status.clone()
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        finish(&self.0, Instant::now(), None);
    }
}

enum Source {
    Pattern(Pattern),
    File {
        file: File,
        frames: u64,
        next_index: u64,
    },
    Pipe(io::Stdin),
}

pub fn spawn(spec: Spec, wake: impl Fn() + Send + Sync + 'static) -> io::Result<Feed> {
    spawn_inner(spec, wake, true)
}

pub fn spawn_demo(spec: Spec, wake: impl Fn() + Send + Sync + 'static) -> io::Result<Feed> {
    spawn_inner(spec, wake, false)
}

fn spawn_inner(
    spec: Spec,
    wake: impl Fn() + Send + Sync + 'static,
    finite: bool,
) -> io::Result<Feed> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid source size, fps, or duration",
        )
    };
    let byte_len = rgba_len(spec.width, spec.height).ok_or_else(invalid)?;
    if !(1..=240).contains(&spec.fps) || !(1..=600).contains(&spec.seconds) || spec.warmup > 60 {
        return Err(invalid());
    }
    let raw_pipe = spec
        .raw
        .as_ref()
        .is_some_and(|path| path.as_os_str() == "-");
    let source = match spec.raw.as_ref() {
        Some(_) if raw_pipe => Source::Pipe(io::stdin()),
        Some(path) => {
            let file = File::open(path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.len() == 0 || metadata.len() % byte_len as u64 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "raw file must contain whole RGBA frames",
                ));
            }
            Source::File {
                file,
                frames: metadata.len() / byte_len as u64,
                next_index: 0,
            }
        }
        None => Source::Pattern(Pattern::new(spec.width, spec.height).ok_or_else(invalid)?),
    };
    let origin = Instant::now();
    {
        let mut stderr = io::stderr().lock();
        writeln!(stderr, "viewer_source_started")?;
        stderr.flush()?;
    }
    let started = origin + Duration::from_secs(u64::from(spec.warmup));
    let deadline = finite.then(|| started + Duration::from_secs(u64::from(spec.seconds)));
    let slot = Arc::new(Mutex::new(Slot {
        status: Status {
            started: Some(started),
            ..Status::default()
        },
        latest: None,
    }));
    let wake = Arc::new(wake);
    if raw_pipe && let Some(deadline) = deadline {
        // A slow or disconnected pipe must not keep the GUI open forever. The
        // blocked read does not hold the mailbox lock or prevent process exit.
        let slot = Arc::clone(&slot);
        let wake = Arc::clone(&wake);
        thread::Builder::new()
            .name("rgba-pipe-timeout".into())
            .spawn(move || {
                thread::sleep(deadline.saturating_duration_since(Instant::now()));
                if finish(&slot, deadline, None) {
                    wake();
                }
            })?;
    }
    let output = Arc::clone(&slot);
    thread::Builder::new()
        .name("rgba-producer".into())
        .spawn(move || {
            let result = produce(
                source,
                &output,
                &*wake,
                Clock {
                    origin,
                    started,
                    deadline,
                    fps: u64::from(spec.fps),
                    byte_len,
                },
            );
            let finished = Instant::now();
            let at = deadline.map_or(finished, |end| end.min(finished));
            if finish(&output, at, result.err().map(|err| err.to_string())) {
                wake();
            }
        })?;
    Ok(Feed(slot))
}

struct Clock {
    origin: Instant,
    started: Instant,
    deadline: Option<Instant>,
    fps: u64,
    byte_len: usize,
}

fn produce(
    mut source: Source,
    slot: &Mutex<Slot>,
    wake: &impl Fn(),
    clock: Clock,
) -> io::Result<()> {
    let mut sequence = 0u64;
    let mut buffer = vec![0; clock.byte_len];
    let mut last_raw = Vec::new();
    let first_measured_slot = (clock.started - clock.origin).as_secs() * clock.fps;
    loop {
        let due = clock.origin + Duration::from_nanos(sequence * 1_000_000_000 / clock.fps);
        thread::sleep(due.saturating_duration_since(Instant::now()));
        let now = Instant::now();
        if clock.deadline.is_some_and(|end| now >= end)
            || slot.lock().unwrap().status.ended.is_some()
        {
            return Ok(());
        }
        let current =
            ((now - clock.origin).as_nanos() * u128::from(clock.fps) / 1_000_000_000) as u64;
        let current = current.max(sequence);
        let late = current.saturating_sub(sequence.max(first_measured_slot));
        sequence = current + 1;
        let filled = match &mut source {
            Source::Pattern(pattern) => {
                pattern.write_rgba(current, &mut buffer);
                true
            }
            Source::File {
                file,
                frames,
                next_index,
            } => {
                let index = current % *frames;
                if index != *next_index {
                    file.seek(SeekFrom::Start(index * clock.byte_len as u64))?;
                }
                if !read_rgba(file, &mut buffer)? {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "raw frame file shrank during playback",
                    ));
                }
                *next_index = index + 1;
                true
            }
            Source::Pipe(reader) => read_rgba(reader, &mut buffer)?,
        };
        if !filled {
            return Ok(());
        }
        let duplicate = !matches!(&source, Source::Pattern(_)) && last_raw == buffer;
        if !matches!(&source, Source::Pattern(_)) && !duplicate {
            last_raw.resize(clock.byte_len, 0);
            last_raw.copy_from_slice(&buffer);
        }
        let produced_at = Instant::now();
        let mut state = slot.lock().unwrap();
        if clock.deadline.is_some_and(|end| produced_at >= end) || state.status.ended.is_some() {
            return Ok(());
        }
        let measured = produced_at >= clock.started;
        state.status.late_slots += late;
        if duplicate {
            state.status.duplicates += u64::from(measured);
            continue;
        }
        let previous = state.latest.replace(Frame {
            sequence: current,
            measured,
            produced_at,
            rgba: buffer,
        });
        state.status.replaced += u64::from(previous.as_ref().is_some_and(|frame| frame.measured));
        state.status.produced += u64::from(measured);
        drop(state);
        wake();
        buffer = previous
            .map(|frame| frame.rgba)
            .unwrap_or_else(|| vec![0; clock.byte_len]);
    }
}

fn finish(slot: &Mutex<Slot>, at: Instant, error: Option<String>) -> bool {
    let mut slot = slot.lock().unwrap();
    if slot.status.ended.is_some() {
        return false;
    }
    slot.status.ended = Some(at);
    slot.status.error = error;
    true
}
